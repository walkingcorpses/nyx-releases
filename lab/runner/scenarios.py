"""scenarios.py - the leak lab runner's entry point. Prepares the lab (WireGuard keys, exit and
client configs, the test torrent), pre-seeds client-a with VPN profile A already active, drives
the scenarios over `docker exec ... nyx-daemon ctl`, runs the analyzer on the capture and the
server logs, and writes /out/report.md. Exit code: non-zero on any finding or scenario failure.

Scenarios:
  cold_start       the daemon starts with profile A active; until the tunnel is up nothing but
                   WireGuard to exit A may leave the client.
  steady_download  a 20 MB torrent is downloaded from the lab seeder via the lab tracker and its
                   SHA-256 checked; the egress and identity checks cover the whole transfer."""
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
EXITS = {"wg-exit-a": "10.88.0.10", "wg-exit-b": "10.88.0.11"}
WG_PORT = 51820
LAB_DNS = "10.88.0.23"
TRACKERS = [["udp://10.88.0.20:6969/announce"], ["http://10.88.0.20:8000/announce"]]
PAYLOAD_SIZE = 20 * 1024 * 1024
DOWNLOAD_TIMEOUT = 600
DWELL = 60


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


def seed_settings(client, conf, exit_name):
    """Pre-seeds the client's settings.json with one profile (the exit's) already active, kill
    switch on, discovery left at its default. Written last: the client entry script starts the
    daemon as soon as this file appears. The sealed form on Linux is "plain:" + base64."""
    endpoint = f"{EXITS[exit_name]}:{WG_PORT}"
    profile = {"id": "a0a0a0a0a0a0a0a0", "name": exit_name, "group": "lab",
               "sealed_conf": "plain:" + base64.b64encode(conf.encode()).decode(),
               "endpoint": endpoint, "endpoint_ip": endpoint}
    settings = {"kill_switch": True, "network": {"profiles": [profile], "active_profile": profile["id"]}}
    write(f"{CONF}/{client}.settings.json.tmp", json.dumps(settings, indent=2))
    os.replace(f"{CONF}/{client}.settings.json.tmp", f"{CONF}/{client}.settings.json")


def cold_start(client, pcap):
    """Starts the client daemon (by seeding its settings) and waits for the tunnel. Returns
    (t_seed, t_up, detail)."""
    t_seed = time.time()
    seed_settings(client, setup_state["confs"]["wg-exit-a"], "wg-exit-a")
    wait_for("client control port", lambda: ctl(client, {"cmd": "status"}), 60)
    st = wait_for("route up", lambda: (s := ctl(client, {"cmd": "status"}))["route"] == "up" and s, 90)
    t_up = time.time()
    # The capture lands a moment after the packets; give it a few seconds before calling it broken.
    try:
        n = wait_for("WireGuard packets in the capture",
                     lambda: analyzer.count_udp_to(pcap, (EXITS["wg-exit-a"], WG_PORT), t_seed - 1, t_up), 10)
    except TimeoutError:
        raise RuntimeError("route is up but the capture holds no WireGuard packet to exit A: capture broken")
    # Exercise the new control commands: import profile B (for rotation later) and list both.
    ctl(client, {"cmd": "import_profile", "path": f"{CONF}/{client}-wg-exit-b.conf", "name": "wg-exit-b", "group": "lab"})
    profiles = ctl(client, {"cmd": "list_profiles"})
    active = [p["name"] for p in profiles if p["active"]]
    if len(profiles) != 2 or active != ["wg-exit-a"]:
        raise RuntimeError(f"unexpected profiles after import: {profiles}")
    return t_seed, t_up, f"route up after {t_up - t_seed:.1f}s ({st['profile_name']}), {n} WireGuard packets before; profile B imported"


def steady_download(client):
    """Seeder seeds the lab torrent directly; the client downloads it through its tunnel. Checks
    the SHA-256 of the downloaded file. Returns a detail line."""
    ctl("seeder", {"cmd": "add_torrent", "path": f"{CONF}/lab.torrent", "save_path": f"{CONF}/seed"})
    wait_for("seeder to verify the payload", lambda: all(t["progress"] >= 1.0 for t in ctl("seeder", {"cmd": "list"})), 120, 1)
    t0 = time.time()
    ctl(client, {"cmd": "add_torrent", "path": f"{CONF}/lab.torrent", "save_path": "/data/dl"})

    def done():
        """True once the client's only torrent is complete; prints progress as it goes."""
        t = ctl(client, {"cmd": "list"})[0]
        print(f"  download {t['progress'] * 100:5.1f}% {t['status']} peers={t['peers']} {t['down_rate'] // 1024} KiB/s", flush=True)
        if t.get("error"):
            raise RuntimeError(f"torrent error: {t['error']}")
        return t["progress"] >= 1.0

    wait_for("download to finish", done, DOWNLOAD_TIMEOUT, 3)
    secs = time.time() - t0
    got = subprocess.run(["docker", "exec", container(client), "sha256sum", "/data/dl/lab-20MB.bin"],
                         capture_output=True, text=True, timeout=60).stdout.split()[0]
    if got != setup_state["sha256"]:
        raise RuntimeError(f"hash mismatch: {got} != {setup_state['sha256']}")
    return f"20 MB in {secs:.0f}s, SHA-256 verified"


def read_logs():
    """All server log entries from /out/*.log (JSON lines), skipping unreadable lines."""
    out = []
    for path in sorted(glob.glob(f"{OUT}/*.log")):
        with open(path, encoding="utf-8") as f:
            for line in f:
                try:
                    out.append(json.loads(line))
                except ValueError:
                    pass
    return out


def observations(logs, client):
    """Notes that are not pass/fail but matter for later checks: what the tracker and the observer
    learned about the client (peer_id, key, User-Agent, version) and the UDP ASSOCIATE port."""
    notes = []
    exits = set(EXITS.values())
    for e in logs:
        if e["server"] == "tracker" and e["src"] in exits | {analyzer.GATEWAY_IDENTITY}:
            notes.append(f"tracker {e.get('proto')} announce from {e['src']}: peer_id={e.get('peer_id')} key={e.get('key')} "
                         f"port={e.get('port')} ip={e.get('ip') or '-'} event={e.get('event')} UA={e.get('user_agent', '-')}")
        if e["server"] == "observer" and e["src"] in exits | {analyzer.GATEWAY_IDENTITY}:
            notes.append(f"observer saw {e['src']}: peer_id={e.get('peer_id')} v={e.get('v')} p={e.get('p')} "
                         f"reserved={e.get('reserved')} encrypted={e.get('encrypted')} error={e.get('error', '-')}")
    logs_txt = subprocess.run(["docker", "logs", container(client)], capture_output=True, text=True).stderr
    assoc = sorted({l.strip() for l in logs_txt.splitlines() if "UDP ASSOCIATE" in l})
    notes.append("SOCKS UDP ASSOCIATE (from NYX_LAB_DEBUG): " + ("; ".join(assoc) if assoc else "no UDP ASSOCIATE seen in this run"))
    counts = {}
    for n in notes:  # repeated connections give identical lines: keep one, with a count
        counts[n] = counts.get(n, 0) + 1
    return [n if c == 1 else f"{n} (x{c})" for n, c in counts.items()]


setup_state = {}


def main():
    """Runs setup, both scenarios and the checks; writes the report; exits with its code."""
    client, pcap = "client-a", f"{OUT}/client-a.pcap"
    allowed = {(EXITS["wg-exit-a"], WG_PORT)}
    scenarios, findings, windows = [], [], {}
    setup_state["confs"] = setup_wireguard(client)
    setup_state["sha256"] = torrentgen.make_payload(f"{CONF}/seed/lab-20MB.bin", PAYLOAD_SIZE)
    torrentgen.make_torrent(f"{CONF}/seed/lab-20MB.bin", f"{CONF}/lab.torrent", TRACKERS)
    for name in EXITS:
        wait_for(f"{name} to be up", lambda n=name: os.path.exists(f"{CONF}/{n}.ready"), 120)
    wait_for("the client capture to start", lambda: os.path.getsize(pcap) >= 24, 60)
    print("lab ready", flush=True)

    t_seed = t_up = time.time()
    try:
        t_seed, t_up, detail = cold_start(client, pcap)
        scenarios.append(("cold_start", True, detail))
    except Exception as e:
        traceback.print_exc()
        scenarios.append(("cold_start", False, str(e)))
    windows["cold_start"] = (0.0, t_up)  # from the first captured packet, before the daemon starts
    print(scenarios[-1], flush=True)

    try:
        scenarios.append(("steady_download", True, steady_download(client)))
    except Exception as e:
        traceback.print_exc()
        scenarios.append(("steady_download", False, str(e)))
    # Dwell while seeding: the transfer takes seconds, and periodic or delayed traffic (DHT
    # bootstrap, LSD, UPnP/NAT-PMP, re-announces) must get a chance to show in the capture.
    time.sleep(DWELL)
    windows["steady_download"] = (t_up, float("inf"))
    print(scenarios[-1], flush=True)

    logs = read_logs()
    for name, (since, until) in windows.items():
        findings += analyzer.check_egress(pcap, allowed, name, since, until)
        findings += analyzer.check_identity([e for e in logs if since <= e.get("ts", 0) <= until], name)
    code = report.write(f"{OUT}/report.md", scenarios, findings, observations(logs, client))
    print(open(f"{OUT}/report.md", encoding="utf-8").read(), flush=True)
    for path in glob.glob(f"{OUT}/*"):  # live volume -> lab/out on the host
        shutil.copy(path, HOST_OUT)
    sys.exit(code)


if __name__ == "__main__":
    main()
