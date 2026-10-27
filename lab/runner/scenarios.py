"""scenarios.py - the leak lab runner's entry point. Prepares the lab (WireGuard keys, exit and
client configs, the test torrents), pre-seeds client-a with VPN profile A already active, drives
the scenarios over `docker exec ... nyx-daemon ctl`, then stops the capture and runs the analyzer
on the finished capture and the server logs, and writes /out/report.md. Exit code: non-zero on any
finding, any scenario that could not run, or a blind positive control.

Every scenario gets a time window; the windows are contiguous, so every captured packet and every
server log line belongs to exactly one scenario. In each window only WireGuard to exit A may leave
the client (check_egress) and the lab servers may only see the exits (check_identity). Windows of
30 s or more must also prove the capture was live (capture_live).

Scenarios (in run order):
  cold_start                 the daemon starts with profile A active; until the tunnel is up nothing
                             but WireGuard to exit A may leave the client.
  steady_download            20 MB from the lab seeder via the lab tracker (IP literals), SHA-256
                             checked, then a dwell while seeding.
  hostname_trackers          a torrent whose trackers are hostnames (tracker.lab): the lab dns must
                             see the lookups only from the exits, never from the real address.
  metadata_requests          RSS feed, search provider and update check pointed at the canary by
                             hostname: every canary request must come from an exit.
  bait                       the tracker hands out home-LAN, private and IPv6 peer addresses and
                             redirects to the canary by IP: none of it may be dialled outside the
                             tunnel.
  discovery_attempts         a long dwell with discovery at its default (LSD, UPnP, NAT-PMP, DHT on).
  tunnel_killed_midtransfer  WireGuard to exit A is dropped mid-download: after the kill nothing
                             but WireGuard may leave, torrents must be held within the stale
                             threshold + 10 s and stay held while cut (exit A keeps sending
                             handshake initiations: finding F1), and after the drop is lifted the
                             tunnel recovers.
  tunnel_cut_both_ways       the same with exit A dropping WireGuard in both directions.
  tunnel_cut_long            the one-way cut held for 130 s, past wireguard-go giving up its
                             handshake attempts (~90 s): held all through, then recovery.
  shutdown_and_crash         a clean `shutdown`, then a SIGKILL of the daemon mid-transfer; each
                             restart must be a clean cold start again.
  ipv6_icmp_broadcast        IPv6 is on in the LAN: a download must produce no IPv6, ICMP,
                             broadcast or mDNS traffic.
  positive_control (last)    deliberate leaks that the lab must detect, or the run fails "blind"."""
import base64
import glob
import json
import os
import shutil
import subprocess
import sys
import time
import traceback

from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.x25519 import X25519PrivateKey

import analyzer
import report
import torrentgen

OUT, CONF, HOST_OUT = "/out", "/conf", "/host-out"
PROJECT = "nyx-lab"
CLIENT = "client-a"
PCAP = f"{OUT}/client-a.pcap"
EXITS = {"wg-exit-a": "10.88.0.10", "wg-exit-b": "10.88.0.11"}
EXIT_IPS = set(EXITS.values())
WG_PORT = 51820
EXIT_A = (EXITS["wg-exit-a"], WG_PORT)
LAB_DNS = "10.88.0.23"
MB = 1024 * 1024
IP_TRACKERS = [["udp://10.88.0.20:6969/announce"], ["http://10.88.0.20:8000/announce"]]
HOST_TRACKERS = [["udp://tracker.lab:6969/announce"], ["http://tracker.lab:8000/announce"]]
# name -> (payload size, payload seed, tracker tiers). Payload file /conf/seed/<name>.bin.
TORRENTS = {
    "lab": (20 * MB, 7, IP_TRACKERS),
    "host": (2 * MB, 9, HOST_TRACKERS),
    "bait-redirect": (1 * MB, 10, [["http://10.88.0.20:8000/bait/redirect"]]),
    "bait-peers": (1 * MB, 11, [["http://10.88.0.20:8000/bait/peers"]]),
    "kill": (20 * MB, 8, IP_TRACKERS),
    "kill2": (20 * MB, 14, IP_TRACKERS),
    "kill3": (20 * MB, 15, IP_TRACKERS),
    "crash": (20 * MB, 12, IP_TRACKERS),
    "v6": (8 * MB, 13, IP_TRACKERS),
}
SLOW = 256 * 1024       # seeder upload cap (B/s) for the mid-transfer scenarios: 20 MB takes ~80 s
DOWNLOAD_TIMEOUT = 600
DWELL = 30              # seeding dwell after the steady download
DISCOVERY_DWELL = 90    # longest practical dwell; libtorrent's LSD interval (5 min) is longer
HELD_WITHIN = 10        # torrents must be held within the stale threshold + this
RECOVER_WITHIN = 60     # tunnel up and torrents released within this after the drop is lifted
DROP_RULE = ["INPUT", "-p", "udp", "--dport", str(WG_PORT), "-j", "DROP"]
DROP_OUT_RULE = ["OUTPUT", "-p", "udp", "--sport", str(WG_PORT), "-j", "DROP"]  # with DROP_RULE: both ways
LONG_CUT = 130          # tunnel_cut_long: > wireguard-go's 90 s of handshake attempts + the 20 s stale threshold
CONTROL = "positive_control"

# Run state shared by the scenarios: windows are (scenario, since, until, kind) with kind
# "normal" or "kill" (the kill window's egress is checked by check_kill_timing).
LAB = {"windows": [], "findings": [], "scenarios": [], "notes": [], "kills": [], "mark": 0.0}
setup_state = {}


def container(service):
    """Compose container name of a lab service (fixed project name, first replica)."""
    return f"{PROJECT}-{service}-1"


def ctl(service, req, timeout=60):
    """Sends one control request to the nyx-daemon in `service` and returns the `ok` value.
    Raises RuntimeError on an `err` reply or if the daemon cannot be reached."""
    p = subprocess.run(["docker", "exec", container(service), "nyx-daemon", "ctl", json.dumps(req)],
                       capture_output=True, text=True, timeout=timeout)
    if p.returncode != 0 or not p.stdout.strip():
        raise RuntimeError(f"ctl {service} {req.get('cmd')}: {p.stdout.strip()} {p.stderr.strip()}")
    return json.loads(p.stdout)["ok"]


def sh(service, script, timeout=60, check=True):
    """Runs `sh -c script` in a lab container; returns stdout. Raises on a non-zero exit if `check`."""
    p = subprocess.run(["docker", "exec", container(service), "sh", "-c", script], capture_output=True, text=True, timeout=timeout)
    if check and p.returncode != 0:
        raise RuntimeError(f"{service}: {script!r} failed: {p.stderr.strip()}")
    return p.stdout


def wait_for(what, pred, timeout, every=0.5):
    """Polls `pred` until it returns a truthy value (returned) or `timeout` seconds pass (raises).
    Exceptions from `pred` count as not ready yet."""
    end, last = time.time() + timeout, None
    while time.time() < end:
        try:
            v = pred()
            if v:
                return v
        except Exception as e:  # not ready yet; keep the last error for the timeout message
            last = e
        time.sleep(every)
    raise TimeoutError(f"timed out after {timeout}s waiting for {what} (last error: {last})")


def keypair():
    """A WireGuard (X25519) key pair as base64 strings: (private, public)."""
    k = X25519PrivateKey.generate()
    raw = serialization.Encoding.Raw
    priv = k.private_bytes(raw, serialization.PrivateFormat.Raw, serialization.NoEncryption())
    pub = k.public_key().public_bytes(raw, serialization.PublicFormat.Raw)
    return base64.b64encode(priv).decode(), base64.b64encode(pub).decode()


def write(path, text):
    """Writes a text file, creating its folder."""
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)


def setup_wireguard(client):
    """Generates keys for both exits and `client`; writes each exit's wg config (/conf/<exit>.wg,
    read by wg-exit/entry.sh) and the client's profiles (/conf/<client>-<exit>.conf).
    Returns {exit: client .conf text}."""
    confs = {}
    for name, ip in EXITS.items():
        exit_priv, exit_pub = keypair()
        cli_priv, cli_pub = keypair()
        write(f"{CONF}/{name}.wg", f"[Interface]\nPrivateKey = {exit_priv}\nListenPort = {WG_PORT}\n\n"
                                   f"[Peer]\nPublicKey = {cli_pub}\nAllowedIPs = 10.100.0.2/32\n")
        conf = (f"[Interface]\nPrivateKey = {cli_priv}\nAddress = 10.100.0.2/32\nDNS = {LAB_DNS}\n\n"
                f"[Peer]\nPublicKey = {exit_pub}\nAllowedIPs = 0.0.0.0/0, ::/0\nEndpoint = {ip}:{WG_PORT}\nPersistentKeepalive = 25\n")
        write(f"{CONF}/{client}-{name}.conf", conf)
        confs[name] = conf
    return confs


def setup_torrents():
    """Writes every test payload and its .torrent (/conf/<name>.torrent); remembers each SHA-256
    and info-hash."""
    for name, (size, seed, trackers) in TORRENTS.items():
        data = f"{CONF}/seed/{name}.bin"
        setup_state[f"sha256:{name}"] = torrentgen.make_payload(data, size, seed)
        setup_state[f"hash:{name}"] = torrentgen.make_torrent(data, f"{CONF}/{name}.torrent", trackers)


def seed_settings(client, conf, exit_name):
    """Pre-seeds the client's settings.json with one profile (the exit's) already active, kill
    switch on, discovery left at its default. The queue limits are raised so the lab's seven
    torrents are never held by the queue (that would read like the kill switch). Written last:
    the client entry script starts the daemon as soon as this file appears. The sealed form on
    Linux is "plain:" + base64."""
    endpoint = f"{EXITS[exit_name]}:{WG_PORT}"
    profile = {"id": "a0a0a0a0a0a0a0a0", "name": exit_name, "group": "lab",
               "sealed_conf": "plain:" + base64.b64encode(conf.encode()).decode(),
               "endpoint": endpoint, "endpoint_ip": endpoint}
    settings = {"kill_switch": True, "max_active_downloads": 50, "max_active_seeds": 50,
                "network": {"profiles": [profile], "active_profile": profile["id"]}}
    write(f"{CONF}/{client}.settings.json.tmp", json.dumps(settings, indent=2))
    os.replace(f"{CONF}/{client}.settings.json.tmp", f"{CONF}/{client}.settings.json")


def change_settings(service, **values):
    """Reads a daemon's settings, changes the given top-level fields and writes them back."""
    s = ctl(service, {"cmd": "get_settings"})
    s.update(values)
    ctl(service, {"cmd": "set_settings", "settings": s})


def logs():
    """The server log entries so far (unparseable lines are reported at the end, not here)."""
    return analyzer.load_logs(sorted(glob.glob(f"{OUT}/*.log")))[0]


def torrent(name):
    """The client's row for test torrent `name`, or None."""
    return next((t for t in ctl(CLIENT, {"cmd": "list"}) if t["name"] == f"{name}.bin"), None)


def add(service, name, save):
    """Adds test torrent `name` to a daemon, saving into `save`."""
    ctl(service, {"cmd": "add_torrent", "path": f"{CONF}/{name}.torrent", "save_path": save})


def seed(name):
    """The seeder adds test torrent `name` from /conf/seed and finishes verifying it."""
    add("seeder", name, f"{CONF}/seed")
    wait_for(f"seeder to verify {name}", lambda: any(t["name"] == f"{name}.bin" and t["progress"] >= 1.0
                                                     for t in ctl("seeder", {"cmd": "list"})), 120, 1)


def wait_done(name):
    """Waits until the client's torrent `name` is complete, printing progress; checks its SHA-256.
    Returns the seconds it took."""
    t0 = time.time()

    def done():
        """True once the torrent is complete; raises on a torrent error."""
        t = torrent(name)
        print(f"  {name} {t['progress'] * 100:5.1f}% {t['status']} peers={t['peers']} {t['down_rate'] // 1024} KiB/s", flush=True)
        if t.get("error"):
            raise RuntimeError(f"torrent error: {t['error']}")
        return t["progress"] >= 1.0

    wait_for(f"{name} to finish", done, DOWNLOAD_TIMEOUT, 3)
    got = sh(CLIENT, f"sha256sum /data/dl/{name}.bin").split()[0]
    if got != setup_state[f"sha256:{name}"]:
        raise RuntimeError(f"{name}: hash mismatch: {got} != {setup_state[f'sha256:{name}']}")
    return time.time() - t0


def wait_midway(name):
    """Waits until the client's torrent `name` has started transferring (3 %), and checks it is
    not nearly done, so the next step really happens mid-transfer. Returns the progress."""
    p = wait_for(f"{name} to start transferring", lambda: (t := torrent(name)) and t["progress"] >= 0.03 and t["progress"], 120, 1)
    if p > 0.9:
        raise RuntimeError(f"{name} was already {p:.0%} done: not a mid-transfer test")
    return p


def run(name, fn, kind="normal"):
    """Runs one scenario from the end of the previous window (LAB['mark']) to now and records it.
    `fn(t0)` returns a detail line or raises (the scenario could not run: an infrastructure
    failure, not a leak). With kind="custom" the scenario records its own windows."""
    t0 = LAB["mark"]
    try:
        ok, detail = True, fn(t0)
    except Exception as e:
        traceback.print_exc()
        ok, detail = False, f"{type(e).__name__}: {e}"
    t1 = time.time()
    if kind == "normal":
        LAB["windows"].append((name, t0, t1, "normal"))
    # A custom scenario's last window ends the run's coverage so far (no gap before the next one).
    LAB["mark"] = LAB["windows"][-1][2] if kind == "custom" and LAB["windows"] else t1
    LAB["scenarios"].append((name, ok, detail))
    print(LAB["scenarios"][-1], flush=True)


def cold_start(_t0):
    """Starts the client daemon (by seeding its settings) and waits for the tunnel. Its window
    starts at the first captured packet (0), before the daemon runs."""
    t_seed = time.time()
    seed_settings(CLIENT, setup_state["confs"]["wg-exit-a"], "wg-exit-a")
    wait_for("client control port", lambda: ctl(CLIENT, {"cmd": "status"}), 60)
    st = wait_for("route up", lambda: (s := ctl(CLIENT, {"cmd": "status"}))["route"] == "up" and s, 90)
    t_up = time.time()
    # The capture lands a moment after the packets; give it a few seconds before calling it broken.
    try:
        n = wait_for("WireGuard packets in the capture", lambda: analyzer.count_udp_to(PCAP, EXIT_A, t_seed - 1, t_up), 10)
    except TimeoutError:
        raise RuntimeError("route is up but the capture holds no WireGuard packet to exit A: capture broken")
    # Exercise the control commands: import profile B (for rotation later) and list both.
    ctl(CLIENT, {"cmd": "import_profile", "path": f"{CONF}/{CLIENT}-wg-exit-b.conf", "name": "wg-exit-b", "group": "lab"})
    profiles = ctl(CLIENT, {"cmd": "list_profiles"})
    active = [p["name"] for p in profiles if p["active"]]
    if len(profiles) != 2 or active != ["wg-exit-a"]:
        raise RuntimeError(f"unexpected profiles after import: {profiles}")
    # The entry script drops NET_ADMIN (capability bit 12) before the daemon starts: prove it.
    cap = int(sh(CLIENT, "grep CapEff /proc/$(cat /run/nyx-daemon.pid)/status").split()[1], 16)
    if cap & (1 << 12):
        raise RuntimeError(f"the daemon still has NET_ADMIN (CapEff {cap:x})")
    return f"route up after {t_up - t_seed:.1f}s ({st['profile_name']}), {n} WireGuard packets before; profile B imported; daemon without NET_ADMIN"


def steady_download(_t0):
    """Seeder seeds the lab torrent directly; the client downloads it through its tunnel; then a
    dwell while seeding (periodic traffic gets a chance to show)."""
    change_settings("seeder", max_active_seeds=50, max_active_downloads=50)
    seed("lab")
    add(CLIENT, "lab", "/data/dl")
    secs = wait_done("lab")
    time.sleep(DWELL)
    return f"20 MB in {secs:.0f}s, SHA-256 verified, {DWELL} s seeding dwell"


def hostname_trackers(t0):
    """Adds a torrent whose trackers are the hostname tracker.lab. The name must be resolved
    through the tunnel (the profile's DNS, reached via the exit): the lab dns may see the lookup
    only from an exit. Identity findings cover any other source; this scenario fails (not
    exercised) if no lookup happened at all."""
    add(CLIENT, "host", "/data/dl")

    def lookups():
        """dns log entries for tracker.lab since the scenario started."""
        return [e for e in logs() if e["server"] == "dns" and e["ts"] >= t0 and str(e.get("name", "")).lower().rstrip(".") == "tracker.lab"]

    try:
        wait_for("a tracker.lab DNS lookup", lookups, 60, 1)
    except TimeoutError:
        raise RuntimeError("no DNS lookup of tracker.lab within 60 s: the hostname tracker was not exercised")
    ih = setup_state["hash:host"]
    try:
        wait_for("an announce for the hostname torrent", lambda: [e for e in logs() if e["server"] == "tracker" and e.get("info_hash") == ih], 30, 1)
        announced = "announce received"
    except TimeoutError:
        announced = "no announce reached the tracker"
    time.sleep(5)
    seen = lookups()
    srcs = sorted({e["src"] for e in seen})
    for e in seen:  # a lab server address passes the identity allowlist but is not a tunnel exit
        if e["src"] not in EXIT_IPS and e["src"] in analyzer.ALLOWED_SOURCES:
            LAB["findings"].append(analyzer.Finding("dns_path", "hostname_trackers", f"tracker.lab looked up from {e['src']}, not an exit"))
    return f"{len(seen)} tracker.lab lookups from {srcs}; {announced}"


def metadata_requests(t0):
    """Points the RSS feed, the search provider and (through the lab dns) the update check at the
    canary by hostname, then runs each once. Every canary request must come from an exit, and
    each of the three must reach the canary at least once, or the scenario was not exercised."""
    feed = {"name": "lab canary", "url": "http://canary.lab/rss.xml", "filter": "", "category": None, "enabled": True}
    s = ctl(CLIENT, {"cmd": "get_settings"})
    s["rss"]["feeds"] = [feed]
    change_settings(CLIENT, rss=s["rss"], search_url="http://canary.lab/search?q={query}", check_updates=True)
    results = {}
    for cmd, req in (("rss_run", {"cmd": "rss_run"}), ("search", {"cmd": "search", "query": "lab"}),
                     ("check_update", {"cmd": "check_update"})):
        try:
            results[cmd] = json.dumps(ctl(CLIENT, req))[:120]
        except RuntimeError as e:
            results[cmd] = f"error: {e}"[:160]
    time.sleep(3)
    hits = [e for e in logs() if e["server"] == "canary" and e["ts"] >= t0]
    rss = [e for e in hits if str(e.get("path", "")).startswith("/rss")]
    search = [e for e in hits if str(e.get("path", "")).startswith("/search")]
    update = [e for e in hits if e.get("port") == 443]
    LAB["notes"].append(f"metadata_requests results: {results}")
    for e in hits:  # a lab server address passes the identity allowlist but is not a tunnel exit
        if e["src"] not in EXIT_IPS and e["src"] in analyzer.ALLOWED_SOURCES:
            LAB["findings"].append(analyzer.Finding("metadata", "metadata_requests", f"canary request from {e['src']}, not an exit: {e.get('path') or e.get('note')}"))
    if not rss or not search or not update:  # each request must have happened, or nothing was checked
        raise RuntimeError(f"not exercised: canary saw rss={len(rss)} search={len(search)} update={len(update)} ({results})")
    return (f"canary saw rss {len(rss)}, search {len(search)}, update check (:443) {len(update)} requests, "
            f"from {sorted({e['src'] for e in hits})}")


def bait(t0):
    """Adds two bait torrents: one whose tracker redirects to the canary by IP, one whose tracker
    returns LAN, private-IP and IPv6 peers. Then waits for dial attempts. Nothing may leave
    outside the tunnel (the window's egress and identity checks)."""
    add(CLIENT, "bait-redirect", "/data/dl")
    add(CLIENT, "bait-peers", "/data/dl")

    def served(path):
        """Tracker log entries for `path` since the scenario started."""
        return [e for e in logs() if e["server"] == "tracker" and e["ts"] >= t0 and e.get("path") == path]

    try:
        wait_for("both bait announces", lambda: served("/bait/peers") and served("/bait/redirect"), 60, 1)
    except TimeoutError:
        raise RuntimeError(f"bait not served: /bait/peers {len(served('/bait/peers'))}, /bait/redirect {len(served('/bait/redirect'))} announces")
    time.sleep(30)  # libtorrent dials new peers within seconds; leave room for retries
    followed = [e for e in logs() if e["server"] == "canary" and e["ts"] >= t0 and str(e.get("path", "")).startswith("/bait-redirected")]
    peers = torrent("bait-peers")
    return (f"bait served; redirect followed {len(followed)}x from {sorted({e['src'] for e in followed}) or '-'}; "
            f"bait-peers status {peers and peers['status']} peers={peers and peers['peers']}")


def discovery_attempts(_t0):
    """A long quiet dwell with discovery at its default (on): DHT, LSD, UPnP and NAT-PMP must
    send nothing outside the tunnel. LSD announces every 5 min in libtorrent and the app has no
    lab-only way to shorten it, so a first LSD announce on torrent start is covered (every
    earlier window) but a periodic one is not."""
    time.sleep(DISCOVERY_DWELL)
    return f"{DISCOVERY_DWELL} s dwell, discovery at its default (on); LSD's 5 min interval is longer than the dwell"


def drop_count():
    """Packets the DROP rule on exit A has dropped (proof the kill really cut the path)."""
    out = sh("wg-exit-a", "iptables -L INPUT -v -n -x")
    for line in out.splitlines():
        parts = line.split()
        if "DROP" in parts and f"dpt:{WG_PORT}" in line:
            return int(parts[0])
    return 0


def cut_midtransfer(t0, name, torrent_name, rules, cut_for=0):
    """Cuts WireGuard to exit A with iptables `rules` (each one inserted on exit A, removed again
    afterwards) in the middle of a slowed download of `torrent_name`. Records the kill time and
    when every torrent read held (for check_kill_timing), keeps the path cut and samples the lock
    once a second for at least 10 s after the hold and at least `cut_for` s after the kill, then
    lifts the cut and checks the tunnel recovers. Records its own windows: before the kill
    (normal), kill to restore (checked by check_kill_timing) and the recovery (normal)."""
    stale = setup_state["stale_after"]
    marks = {"kill": None, "restore": None, "held": None, "samples": []}

    def lift(check=True):
        """Removes every cut rule from exit A."""
        for r in rules:
            sh("wg-exit-a", "iptables -D " + " ".join(r), check=check)

    try:
        change_settings("seeder", up_limit=SLOW)
        seed(torrent_name)
        add(CLIENT, torrent_name, "/data/dl")
        p = wait_midway(torrent_name)
        age0 = ctl(CLIENT, {"cmd": "network_status"})["handshake_age_secs"]
        for r in rules:
            sh("wg-exit-a", "iptables -I " + " ".join(r))
        marks["kill"] = time.time()
        # Sample the lock about once a second from the kill until the cut is lifted: the first
        # sample with every torrent held sets `held`; from then on the lock must stay on (and the
        # capture gets time to show stray traffic while held). The cut lasts at least `cut_for` s,
        # 10 s past the hold, or (never held) until the hold deadline + 15 s has passed.
        trace = []  # (time, killed, route, handshake age, rx bytes): every sample, for the notes
        while True:
            now = time.time()
            if marks["held"]:
                if now >= max(marks["held"] + 10, marks["kill"] + cut_for):
                    break
            elif now >= max(marks["kill"] + stale + HELD_WITHIN + 15, marks["kill"] + cut_for):
                break
            ns, st, rows = ctl(CLIENT, {"cmd": "network_status"}), ctl(CLIENT, {"cmd": "status"}), ctl(CLIENT, {"cmd": "list"})
            trace.append((now, st["killed"], ns["route"], ns["handshake_age_secs"], ns["rx_bytes"]))
            if not marks["held"] and st["killed"] and all(t["status"] == "queued" for t in rows if t["status"] not in ("error", "paused")):
                marks["held"] = now
            if marks["held"]:
                marks["samples"].append(trace[-1][:4])
            time.sleep(1)
        net = ctl(CLIENT, {"cmd": "network_status"})
        LAB["notes"].append(f"{name} samples while cut (s after kill, killed, route, handshake age, rx bytes): "
                            + ", ".join(f"({t - marks['kill']:.0f}, {k}, {r}, {a}, {x})" for t, k, r, a, x in trace))
        dropped = drop_count()
        if dropped == 0:
            raise RuntimeError("the DROP rule on exit A matched no packet: the tunnel was not cut")
        # Snapshot before lifting the cut: recovery must show rx or progress growing past this.
        before = {"rx": ctl(CLIENT, {"cmd": "network_status"})["rx_bytes"], "progress": torrent(torrent_name)["progress"]}
        lift()
        marks["restore"] = time.time()
        back_at, last = None, None
        end = marks["restore"] + RECOVER_WITHIN + 10
        while time.time() < end:
            ns, st, t = ctl(CLIENT, {"cmd": "network_status"}), ctl(CLIENT, {"cmd": "status"}), torrent(torrent_name)
            last = {"route": ns["route"], "killed": st["killed"], "status": t["status"], "rx": ns["rx_bytes"], "progress": t["progress"]}
            if analyzer.recovered(before, last):
                back_at = time.time()
                break
            time.sleep(1)
        LAB["notes"].append(f"{name} recovery: at restore {before}; "
                            f"{'recovered' if back_at else 'last'} sample {last}")
        LAB["findings"].extend(analyzer.check_recovery(marks["restore"], back_at, RECOVER_WITHIN, name))
        held = f"{marks['held'] - marks['kill']:.1f} s" if marks["held"] else "never"
        back = (f"{back_at - marks['restore']:.1f} s (rx {before['rx']} -> {last['rx']}, "
                f"progress {before['progress']:.0%} -> {last['progress']:.0%})") if back_at else "never"
        return (f"killed at {p:.0%} (handshake age {age0} s); held after {held} (stale {stale} s + {HELD_WITHIN} s allowed); route then "
                f"{net['route']}; cut for {marks['restore'] - marks['kill']:.0f} s; {dropped} WireGuard packets dropped; "
                f"recovered after {back}")
    finally:
        now = time.time()
        if marks["kill"] and not marks["restore"]:  # never leave exit A cut for the later scenarios
            lift(check=False)
            marks["restore"] = time.time()
        LAB["windows"].append((name, t0, marks["kill"] or now, "normal"))
        if marks["kill"]:
            LAB["windows"].append((f"{name} (killed)", marks["kill"], marks["restore"], "kill"))
            LAB["kills"].append((name, marks["kill"], marks["restore"], marks["held"], stale, marks["samples"]))
            LAB["windows"].append((f"{name} (recovery)", marks["restore"], now, "normal"))


def tunnel_killed_midtransfer(t0):
    """One-way cut: exit A drops the client's WireGuard but can still send to it, so exit A's
    handshake initiations keep reaching the client (finding F1: they must not count as a live
    tunnel)."""
    return cut_midtransfer(t0, "tunnel_killed_midtransfer", "kill", [DROP_RULE])


def tunnel_cut_both_ways(t0):
    """Both-ways cut: exit A drops the client's WireGuard in and its own WireGuard out."""
    return cut_midtransfer(t0, "tunnel_cut_both_ways", "kill2", [DROP_RULE, DROP_OUT_RULE])


def tunnel_cut_long(t0):
    """A one-way cut held for LONG_CUT s, longer than wireguard-go's ~90 s of handshake attempts
    plus the stale threshold: the lock must hold all through, also once exit A gives up."""
    return cut_midtransfer(t0, "tunnel_cut_long", "kill3", [DROP_RULE], cut_for=LONG_CUT)


def restart_client():
    """Starts the client daemon again on its data dir (the entry script's supervisor waits for
    /run/nyx-restart) and waits for the tunnel. Returns (seconds to route up, WireGuard packets
    before it, torrents restored)."""
    wait_for("the daemon to be gone", lambda: subprocess.run(
        ["docker", "exec", container(CLIENT), "nyx-daemon", "ctl", '{"cmd":"status"}'], capture_output=True).returncode != 0, 20)
    t = time.time()
    sh(CLIENT, "touch /run/nyx-restart")
    wait_for("client control port after restart", lambda: ctl(CLIENT, {"cmd": "status"}), 60)
    wait_for("route up after restart", lambda: ctl(CLIENT, {"cmd": "status"})["route"] == "up", 90)
    up = time.time()
    n = wait_for("WireGuard packets after restart", lambda: analyzer.count_udp_to(PCAP, EXIT_A, t, up + 1), 10)
    return up - t, n, len(ctl(CLIENT, {"cmd": "list"}))


def shutdown_and_crash(_t0):
    """A clean `shutdown`, a pause, a restart; then a download, SIGKILL of the daemon in the middle
    of it (a crash: no cleanup), a pause, a restart. Nothing may leave outside the tunnel while
    the daemon stops, is down, or starts again (each restart is a cold start)."""
    ctl(CLIENT, {"cmd": "shutdown"})
    time.sleep(5)
    up1, n1, rows1 = restart_client()
    change_settings("seeder", up_limit=SLOW)
    seed("crash")
    add(CLIENT, "crash", "/data/dl")
    p = wait_midway("crash")
    sh(CLIENT, "kill -9 $(cat /run/nyx-daemon.pid)")
    time.sleep(8)
    up2, n2, rows2 = restart_client()
    # The daemon saves resume data once a minute, so a torrent added just before the crash can be
    # gone after it (seen in lab run 1). Not a leak: note it and add it again.
    if torrent("crash") is None:
        LAB["notes"].append("shutdown_and_crash: the torrent added shortly before the SIGKILL was gone after the restart "
                            "(resume data is saved every 60 s); re-added")
        add(CLIENT, "crash", "/data/dl")
    change_settings("seeder", up_limit=0)
    secs = wait_done("crash")
    return (f"clean shutdown: back up in {up1:.1f}s ({n1} WireGuard packets first, {rows1} torrents restored); "
            f"SIGKILL at {p:.0%}: back up in {up2:.1f}s ({n2} WireGuard packets first, {rows2} torrents), "
            f"download finished {secs:.0f}s later, SHA-256 verified")


def ipv6_icmp_broadcast(_t0):
    """IPv6 is enabled on the home LAN, so the client has a ULA address and an IPv6 default route:
    IPv6 leaks are possible. A plain download must still produce no IPv6, ICMP, broadcast or
    mDNS (the window's egress check flags each; neighbour discovery and ARP are housekeeping)."""
    addrs = sh(CLIENT, "ip -6 addr show dev eth0 scope global")
    if "fd77::a" not in addrs:
        raise RuntimeError(f"client has no IPv6 address, so IPv6 leaks could not occur: {addrs.strip()}")
    change_settings("seeder", up_limit=0)
    seed("v6")
    add(CLIENT, "v6", "/data/dl")
    secs = wait_done("v6")
    time.sleep(20)
    return f"client has fd77::a; 8 MB in {secs:.0f}s, SHA-256 verified, 20 s dwell"


def positive_control(_t0):
    """Deliberately leaks from the client's network namespace (bash /dev/tcp and /dev/udp, so the
    client image needs no extra tools): TCP to the tracker, a DNS query to the lab dns, UDP to
    exit A off its WireGuard port, and UDP over IPv6 to the lab gateway. Each must come back as a
    finding, or the lab is blind."""
    # DNS query for control.lab, type A (id 0x1234, recursion desired).
    dns_query = r"\x12\x34\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00\x07control\x03lab\x00\x00\x01\x00\x01"
    script = ("exec 3<>/dev/tcp/10.88.0.20/8000; printf 'GET /positive-control HTTP/1.0\\r\\n\\r\\n' >&3; timeout 3 cat <&3 >/dev/null; "
              # Read the answer before closing: a reply to a closed port makes the kernel send
              # ICMP port-unreachable, which would be a stray finding of the control itself.
              f"exec 4<>/dev/udp/10.88.0.23/53; printf '{dns_query}' >&4; timeout 3 head -c 1 <&4 >/dev/null; exec 4<&-; "
              "printf 'positive-control' > /dev/udp/10.88.0.10/9999; "
              "printf 'positive-control' > /dev/udp/fd77::1/9999")
    p = subprocess.run(["docker", "exec", container(CLIENT), "bash", "-c", script], capture_output=True, text=True, timeout=30)
    if p.returncode != 0:
        raise RuntimeError(f"control leak commands failed: {p.stderr.strip()}")
    time.sleep(3)  # let the capture and the server logs record them
    return "deliberate leaks sent from client-a's namespace"


def observations(entries):
    """Notes that are not pass/fail but matter for later checks: what the tracker and the observer
    learned about the client (peer_id, key, User-Agent, version) and the UDP ASSOCIATE port."""
    notes = list(LAB["notes"])
    exits = EXIT_IPS
    for e in entries:
        if e["server"] == "tracker" and e["src"] in exits | {analyzer.GATEWAY_IDENTITY} and e.get("path") != "/positive-control":
            notes.append(f"tracker {e.get('proto')} {e.get('path') or ''} announce from {e['src']}: peer_id={e.get('peer_id')} "
                         f"key={e.get('key')} port={e.get('port')} ip={e.get('ip') or '-'} event={e.get('event')} UA={e.get('user_agent', '-')}")
        if e["server"] == "observer" and e["src"] in exits | {analyzer.GATEWAY_IDENTITY}:
            notes.append(f"observer saw {e['src']}: peer_id={e.get('peer_id')} v={e.get('v')} p={e.get('p')} "
                         f"reserved={e.get('reserved')} encrypted={e.get('encrypted')} error={e.get('error', '-')}")
    logs_txt = subprocess.run(["docker", "logs", container(CLIENT)], capture_output=True, text=True)
    assoc = sorted({l.strip() for l in (logs_txt.stdout + logs_txt.stderr).splitlines() if "UDP ASSOCIATE" in l})
    notes.append("SOCKS UDP ASSOCIATE (from NYX_LAB_DEBUG): " + ("; ".join(assoc) if assoc else "no UDP ASSOCIATE seen in this run"))
    counts = {}
    for n in notes:  # repeated connections give identical lines: keep one, with a count
        counts[n] = counts.get(n, 0) + 1
    return [n if c == 1 else f"{n} (x{c})" for n, c in counts.items()]


def analyse():
    """Stops the capture (so the pcap is complete), then runs every check over every window and
    writes the report. Returns the exit code."""
    subprocess.run(["docker", "stop", "-t", "5", container("sniff-client-a")], capture_output=True, timeout=60)
    entries, bad = analyzer.load_logs(sorted(glob.glob(f"{OUT}/*.log")))
    findings = list(LAB["findings"]) + bad
    allowed = {EXIT_A}
    failed_live = {}
    for name, since, until, kind in LAB["windows"]:
        if kind == "normal":
            findings += analyzer.check_egress(PCAP, allowed, name, since, until)
        findings += analyzer.check_identity([e for e in entries if since <= e.get("ts", 0) <= until], name)
        base = name.split(" ")[0]
        if base not in ("cold_start", CONTROL) and until - since >= 30:
            live, why = analyzer.capture_live(PCAP, EXIT_A, since, until)
            if not live:
                failed_live.setdefault(base, f"capture not live in {name}: {why}")
    for name, kill, restore, held, stale, samples in LAB["kills"]:
        findings += analyzer.check_kill_timing(PCAP, kill, allowed, stale, held, HELD_WITHIN, until=restore,
                                               scenario=f"{name} (killed)", samples=samples)
    LAB["scenarios"] = [(n, False, failed_live[n]) if ok and n in failed_live else (n, ok, d) for n, ok, d in LAB["scenarios"]]
    real, missing = analyzer.split_control(findings, CONTROL, analyzer.CONTROL_SIGNATURES)
    for m in missing:
        print(f"lab blind: expected finding {m!r} not detected", flush=True)
    code = report.write(f"{OUT}/report.md", LAB["scenarios"], real, observations(entries), missing, partial=setup_state.get("only"))
    print(open(f"{OUT}/report.md", encoding="utf-8").read(), flush=True)
    return code


# The scenarios between cold_start and the control, in run order: (name, function, window kind).
# LAB_ONLY may name any of these (report.lab_only rejects other names).
SCENARIOS = (("steady_download", steady_download, "normal"), ("hostname_trackers", hostname_trackers, "normal"),
             ("metadata_requests", metadata_requests, "normal"), ("bait", bait, "normal"),
             ("discovery_attempts", discovery_attempts, "normal"),
             ("tunnel_killed_midtransfer", tunnel_killed_midtransfer, "custom"),
             ("tunnel_cut_both_ways", tunnel_cut_both_ways, "custom"),
             ("tunnel_cut_long", tunnel_cut_long, "custom"),
             ("shutdown_and_crash", shutdown_and_crash, "normal"),
             ("ipv6_icmp_broadcast", ipv6_icmp_broadcast, "normal"))


def main():
    """Runs setup, every scenario and the checks; writes the report; copies the capture, logs and
    report to lab/out (also when something crashes); exits with the report's code."""
    code = 2
    try:
        # Checked before any setup: an unknown name fails the run at once (report.lab_only).
        only = report.lab_only(os.environ.get("LAB_ONLY", ""), [n for n, _, _ in SCENARIOS])
        setup_state["only"] = ",".join(sorted(only)) or None
        setup_state["confs"] = setup_wireguard(CLIENT)
        setup_torrents()
        setup_state["stale_after"] = int(sh(CLIENT, "printenv NYX_LAB_STALE_AFTER_SECS").strip())
        for name in EXITS:
            wait_for(f"{name} to be up", lambda n=name: os.path.exists(f"{CONF}/{n}.ready"), 120)
        wait_for("the client capture to start", lambda: os.path.getsize(PCAP) >= 24, 60)
        print("lab ready", flush=True)
        run("cold_start", cold_start)  # window from 0: the first captured packet, before the daemon runs
        for name, fn, kind in SCENARIOS:
            if only and name not in only:  # LAB_ONLY: dev runs of a subset (cold start and control always run)
                LAB["notes"].append(f"{name}: skipped (LAB_ONLY)")
                continue
            run(name, fn, kind)
        t_ctl = LAB["mark"]
        run(CONTROL, positive_control, kind="custom")
        LAB["windows"].append((CONTROL, t_ctl, float("inf"), "normal"))  # to the end: late log lines too
        code = analyse()
    finally:
        os.makedirs(HOST_OUT, exist_ok=True)
        for path in glob.glob(f"{OUT}/*"):  # live volume -> lab/out on the host
            try:
                shutil.copy(path, HOST_OUT)
            except OSError as e:
                print(f"could not copy {path}: {e}", flush=True)
    sys.exit(code)


if __name__ == "__main__":
    main()
