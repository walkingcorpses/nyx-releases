//! Tests for the app-level features: kill switch, RSS auto-download, search, torrent URL fetch,
//! queue order and the settings checks that guard them. HTTP goes to a local test server.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::Path;
use std::sync::{Arc, Mutex};

use chrono::Local;
use nyx_core::{Feed, ProxySettings, Settings};
use nyx_engine::testkit::socks5::Socks5Stub;
use nyx_engine::testkit::{free_loopback_addr, make_torrent};
use nyx_app::state::AppState;

/// An app state on loopback with no discovery, storing everything in `dir`.
fn app(dir: &Path) -> AppState {
    AppState::new(dir, free_loopback_addr(), false).unwrap()
}

/// A local HTTP server. `handler` gets the request path and returns the body to send.
/// Returns the base URL (`http://127.0.0.1:port`) and the list of request paths seen.
fn serve(handler: impl Fn(&str) -> Vec<u8> + Send + 'static) -> (String, Arc<Mutex<Vec<String>>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for conn in l.incoming().flatten() {
            let mut c = conn;
            let mut buf = [0u8; 4096];
            let n = c.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).into_owned();
            let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
            log.lock().unwrap().push(path.clone());
            let body = handler(&path);
            let _ = c.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes());
            let _ = c.write_all(&body);
        }
    });
    (format!("http://{addr}"), seen)
}

fn magnet(n: u8) -> String {
    format!("magnet:?xt=urn:btih:{}", format!("{n:02x}").repeat(20))
}

/// With a proxy set and the kill switch on, losing the proxy holds every torrent; getting it back
/// releases exactly those.
#[test]
fn kill_switch_holds_torrents_while_the_proxy_is_down() {
    let tmp = tempfile::tempdir().unwrap();
    let stub = Socks5Stub::start("127.0.0.1:0".parse().unwrap(), None);
    let paddr = stub.addr();
    let mut s = Settings::default();
    s.proxy = Some(ProxySettings { host: "127.0.0.1".into(), port: paddr.port(), username: None, password: None });
    s.save(tmp.path()).unwrap();
    let a = app(tmp.path());
    a.add_magnet(&magnet(1), &tmp.path().join("dl").to_string_lossy(), None, false).unwrap();
    let status = |a: &AppState| a.list_torrents()[0].status;

    a.tick(Local::now().naive_local());
    assert_eq!(status(&a), "metadata");
    assert!(!a.status().killed);

    stub.stop();
    a.probe_proxy();
    a.tick(Local::now().naive_local());
    assert_eq!(status(&a), "queued", "kill switch should hold the torrent");
    let st = a.status();
    assert!(st.proxy_down && st.killed);

    drop(stub);
    let _back = Socks5Stub::start(paddr, None);
    a.probe_proxy();
    a.tick(Local::now().naive_local());
    assert_eq!(status(&a), "metadata", "released when the proxy is back");
    assert!(!a.status().killed && !a.status().proxy_down);
}

/// With the kill switch off nothing is paused when the proxy drops (the engine still never goes direct).
#[test]
fn kill_switch_can_be_turned_off() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = Settings::default();
    s.kill_switch = false;
    s.proxy = Some(ProxySettings { host: "127.0.0.1".into(), port: 1, username: None, password: None });
    s.save(tmp.path()).unwrap();
    let a = app(tmp.path());
    a.add_magnet(&magnet(2), &tmp.path().join("dl").to_string_lossy(), None, false).unwrap();
    a.probe_proxy();
    a.tick(Local::now().naive_local());
    assert_eq!(a.list_torrents()[0].status, "metadata");
    assert!(a.status().proxy_down && !a.status().killed);
}

/// RSS: the first poll of a feed only records what is there; later polls add new matching items once.
#[test]
fn rss_baselines_then_adds_only_new_matching_items() {
    let tmp = tempfile::tempdir().unwrap();
    let body = Arc::new(Mutex::new(String::new()));
    let b2 = body.clone();
    let (base, _) = serve(move |_| b2.lock().unwrap().clone().into_bytes());
    let item = |title: &str, n: u8| format!("<item><title>{title}</title><guid>g{n}</guid><link>{}</link></item>", magnet(n).replace('&', "&amp;"));
    let feed = |items: &[String]| format!("<rss><channel>{}</channel></rss>", items.concat());

    *body.lock().unwrap() = feed(&[item("Ubuntu old", 10)]);
    let a = app(tmp.path());
    let mut s = a.get_settings();
    s.rss.feeds = vec![Feed { name: "f".into(), url: format!("{base}/feed"), filter: "ubuntu".into(), category: None, enabled: true }];
    a.set_settings(s).unwrap();

    let r = a.rss_run();
    assert_eq!((r.added, r.baselined), (0, 1), "{r:?}");
    assert!(a.list_torrents().is_empty(), "the baseline must not download anything");

    *body.lock().unwrap() = feed(&[item("Ubuntu old", 10), item("Ubuntu 99.04 Desktop", 11), item("Fedora 99", 12)]);
    let r = a.rss_run();
    assert_eq!((r.added, r.baselined), (1, 0), "{r:?}");
    assert_eq!(a.list_torrents().len(), 1);
    let r = a.rss_run();
    assert_eq!(r.added, 0, "an item is added only once: {r:?}");
    assert_eq!(a.list_torrents().len(), 1);
}

/// RSS items are remembered across restarts (so a restart never re-adds the world).
#[test]
fn rss_seen_list_survives_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let body = Arc::new(Mutex::new(String::new()));
    let b2 = body.clone();
    let (base, _) = serve(move |_| b2.lock().unwrap().clone().into_bytes());
    let feed = |n: u8| format!("<rss><channel><item><title>Thing {n}</title><guid>g{n}</guid><link>{}</link></item></channel></rss>", magnet(n));
    *body.lock().unwrap() = feed(20);
    let feed_cfg = Feed { name: "f".into(), url: format!("{base}/feed"), filter: String::new(), category: None, enabled: true };
    {
        let a = app(tmp.path());
        let mut s = a.get_settings();
        s.rss.feeds = vec![feed_cfg.clone()];
        a.set_settings(s).unwrap();
        a.rss_run(); // baseline
        *body.lock().unwrap() = feed(21);
        assert_eq!(a.rss_run().added, 1);
    }
    let a = app(tmp.path());
    assert_eq!(a.rss_run().added, 0, "restart must not add item 21 again");
}

/// Search: the query is URL-encoded into the template, and results come back best-seeded first.
#[test]
fn search_uses_the_provider_and_sorts_by_seeders() {
    let tmp = tempfile::tempdir().unwrap();
    let xml = format!(
        r#"<rss xmlns:torznab="x"><channel>
        <item><title>few</title><link>{}</link><torznab:attr name="seeders" value="3"/></item>
        <item><title>many</title><link>{}</link><torznab:attr name="seeders" value="300"/></item></channel></rss>"#,
        magnet(30),
        magnet(31)
    );
    let (base, seen) = serve(move |_| xml.clone().into_bytes());
    let a = app(tmp.path());
    assert!(a.search("x").unwrap_err().contains("no search provider"));
    let mut s = a.get_settings();
    s.search_url = format!("{base}/api?t=search&q={{query}}");
    a.set_settings(s).unwrap();
    let r = a.search("ubuntu linux").unwrap();
    assert_eq!(r.iter().map(|x| x.title.as_str()).collect::<Vec<_>>(), ["many", "few"]);
    assert!(seen.lock().unwrap()[0].contains("q=ubuntu%20linux"), "{:?}", seen.lock().unwrap());
    assert!(a.search("   ").is_err());
}

/// A .torrent behind a link is downloaded, checked and handed back as a file path; junk is refused.
#[test]
fn fetch_torrent_url_checks_what_it_downloads() {
    let tmp = tempfile::tempdir().unwrap();
    let bytes = make_torrent(&tmp.path().join("seed"), 64 * 1024);
    let (base, _) = serve(move |p| if p.contains("good") { bytes.clone() } else { b"<html>not a torrent</html>".to_vec() });
    let a = app(&tmp.path().join("data"));
    let path = a.fetch_torrent_url(&format!("{base}/good.torrent")).unwrap();
    assert!(Path::new(&path).exists());
    assert!(a.inspect_torrent(&path).is_ok());
    assert!(a.fetch_torrent_url(&format!("{base}/junk")).is_err());
    assert!(a.fetch_torrent_url("file:///C:/Windows/win.ini").is_err());
}

/// The queue order is reported, can be changed, and is sent to the UI when it changes.
#[test]
fn queue_order_moves_and_is_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let a = app(tmp.path());
    let dl = tmp.path().join("dl").to_string_lossy().into_owned();
    let ids: Vec<String> = (40..43).map(|n| a.add_magnet(&magnet(n), &dl, None, false).unwrap()).collect();
    let first = a.tick(Local::now().naive_local());
    let order = first.update.queue.expect("order is sent on the first tick");
    assert_eq!(order.len(), 3);
    assert!(a.tick(Local::now().naive_local()).update.queue.is_none(), "unchanged order is not re-sent");

    let last = order[2].clone();
    a.move_in_queue(&last, 0);
    assert_eq!(a.queue_order()[0], last);
    let moved = a.tick(Local::now().naive_local()).update.queue.expect("a move is reported");
    assert_eq!(moved[0], last);
    assert!(ids.contains(&last));
}

/// Settings that would make automation unsafe or broken are refused, and nothing is saved.
#[test]
fn automation_settings_are_validated() {
    let tmp = tempfile::tempdir().unwrap();
    let a = app(tmp.path());
    let base = a.get_settings();
    let mut bad: Vec<Settings> = Vec::new();
    let feed = |url: &str, filter: &str, cat: Option<&str>| Feed {
        name: "f".into(), url: url.into(), filter: filter.into(), category: cat.map(str::to_string), enabled: true,
    };
    for f in [feed("ftp://x", "", None), feed("file:///C:/x", "", None), feed("https://ok.example/rss", "(", None), feed("https://ok.example/rss", "", Some("nope"))] {
        let mut s = base.clone();
        s.rss.feeds = vec![f];
        bad.push(s);
    }
    let mut s = base.clone();
    s.rss.interval_minutes = 1;
    bad.push(s);
    let mut s = base.clone();
    s.search_url = "https://x.example/search".into(); // no {query}
    bad.push(s);
    let mut s = base.clone();
    s.search_url = "ftp://x.example/{query}".into();
    bad.push(s);
    let mut s = base.clone();
    s.blocklist = Some(tmp.path().join("does-not-exist.txt"));
    bad.push(s);
    for s in bad {
        assert!(a.set_settings(s.clone()).is_err(), "should be refused: {s:?}");
    }
    assert!(!tmp.path().join("settings.json").exists(), "a refused change must not be saved");
}

/// A blocklist file is applied, and an unreadable one at startup is a warning, not a crash.
#[test]
fn blocklist_loads_and_a_missing_one_warns() {
    let tmp = tempfile::tempdir().unwrap();
    let list = tmp.path().join("list.txt");
    std::fs::write(&list, "Test:10.0.0.0-10.255.255.255\n").unwrap();
    let a = app(tmp.path());
    let mut s = a.get_settings();
    s.blocklist = Some(list);
    a.set_settings(s).unwrap();
    drop(a);

    // Now a settings file that points at a list that is gone.
    let tmp2 = tempfile::tempdir().unwrap();
    let mut s = Settings::default();
    s.blocklist = Some(tmp2.path().join("gone.txt"));
    s.save(tmp2.path()).unwrap();
    let a = app(tmp2.path());
    assert!(a.status().warnings.iter().any(|w| w.contains("Blocklist")), "{:?}", a.status().warnings);
    let _: SocketAddr = free_loopback_addr();
}

// ---------------------------------------------------------------------------------------------
// Network lock and the VPN tunnel (spec sections 4 and 5). TestPeer is an in-process WireGuard
// server on loopback, so none of these touch the internet.
// ---------------------------------------------------------------------------------------------

use base64::Engine as _;
use nyx_app::netlock::{NetTiming, Route};
use nyx_core::Profile;
use nyx_engine::testkit::wait_until;
use nyx_tunnel::conf::WgConfig;
use nyx_tunnel::testpeer::TestPeer;
use std::time::Duration;

/// What `http_agent` callers get while the lock holds.
const LOCK_ERR: &str = "the network lock is holding: no VPN tunnel";
/// Inside the TestPeer: its TCP echo service.
const ECHO: &str = "10.0.0.1:7";

/// Renders a parsed config back to `.conf` text, the form a profile stores (sealed).
fn conf_text(c: &WgConfig) -> String {
    let b64 = |k: &[u8]| base64::engine::general_purpose::STANDARD.encode(k);
    let addrs: Vec<String> = c.addresses.iter().map(|(ip, p)| format!("{ip}/{p}")).collect();
    let dns: Vec<String> = c.dns.iter().map(|ip| ip.to_string()).collect();
    format!(
        "[Interface]\nPrivateKey = {}\nAddress = {}\nDNS = {}\n\n[Peer]\nPublicKey = {}\nAllowedIPs = 0.0.0.0/0, ::/0\nEndpoint = {}:{}\n",
        b64(&c.private_key),
        addrs.join(", "),
        dns.join(", "),
        b64(&c.peer_public_key),
        c.endpoint.host,
        c.endpoint.port
    )
}

/// The client private key as it appears in the .conf, to check it never reaches the UI.
fn private_key_b64(peer: &TestPeer) -> String {
    base64::engine::general_purpose::STANDARD.encode(peer.client_config().private_key)
}

/// Settings with one profile "Test VPN" (id "p1") for `peer`, sealed from `conf`, and active.
fn vpn_settings(peer: &TestPeer, conf: &str) -> Settings {
    let mut s = Settings::default();
    s.network.profiles.push(Profile {
        id: "p1".into(),
        name: "Test VPN".into(),
        group: String::new(),
        sealed_conf: nyx_app::secret::seal(conf),
        endpoint: peer.endpoint().to_string(),
        endpoint_ip: peer.endpoint().to_string(),
    });
    s.network.active_profile = Some("p1".into());
    s
}

/// Saves `s` into `dir` and starts an app on it with the given lock timing.
fn vpn_app(dir: &Path, s: &Settings, timing: NetTiming) -> AppState {
    s.save(dir).unwrap();
    AppState::new_with_timing(dir, free_loopback_addr(), false, timing).unwrap()
}

/// Ticks until `cond` holds (or `secs` pass).
fn tick_until(a: &AppState, secs: u64, mut cond: impl FnMut(&AppState) -> bool) -> bool {
    wait_until(Duration::from_secs(secs), || {
        a.tick(Local::now().naive_local());
        cond(a)
    })
}

/// With a profile active the engine's proxy is the tunnel's SOCKS port: a peer connection inside
/// the tunnel moves bytes through WireGuard. The UI-facing JSON never carries key material.
#[test]
fn profile_active_points_engine_at_tunnel_socks() {
    let tmp = tempfile::tempdir().unwrap();
    let peer = TestPeer::start();
    let a = vpn_app(tmp.path(), &vpn_settings(&peer, &conf_text(&peer.client_config())), NetTiming::default());
    assert!(wait_until(Duration::from_secs(10), || a.network_status().route == Route::Up), "{:?}", a.network_status());
    let id = a.add_magnet(&magnet(9), &tmp.path().join("dl").to_string_lossy(), None, false).unwrap();
    a.tick(Local::now().naive_local());
    let before = a.network_status().tx_bytes;
    a.engine.add_peer(&id, ECHO.parse().unwrap()).unwrap();
    assert!(
        wait_until(Duration::from_secs(15), || a.network_status().tx_bytes > before),
        "engine traffic should go through the tunnel: {:?}",
        a.network_status()
    );
    let st = a.status();
    assert_eq!((st.route.as_str(), st.profile_name.as_deref(), st.killed, st.proxy_down), ("up", Some("Test VPN"), false, false));
    let ns = a.network_status();
    assert_eq!(ns.profile.as_deref(), Some("Test VPN"));
    assert!(ns.handshake_age_secs.is_some() && ns.rx_bytes > 0);
    let json = serde_json::to_string(&st).unwrap() + &serde_json::to_string(&ns).unwrap();
    assert!(!json.contains(&private_key_b64(&peer)) && !json.contains("PrivateKey"), "{json}");
}

/// Cold start with the lock on: nothing runs and no HTTP goes out until the first handshake.
#[test]
fn cold_start_holds_everything_until_first_handshake() {
    let tmp = tempfile::tempdir().unwrap();
    let peer = TestPeer::start();
    peer.set_paused(true);
    let a = vpn_app(tmp.path(), &vpn_settings(&peer, &conf_text(&peer.client_config())), NetTiming::default());
    a.add_magnet(&magnet(10), &tmp.path().join("dl").to_string_lossy(), None, false).unwrap();
    a.tick(Local::now().naive_local());
    let st = a.status();
    assert!(st.killed, "the lock must hold before the first handshake");
    assert_eq!(st.route, "starting");
    assert_eq!(a.list_torrents()[0].status, "queued");
    assert_eq!(a.check_update().unwrap_err(), LOCK_ERR);
    assert_eq!(a.fetch_torrent_url("http://127.0.0.1:9/x.torrent").unwrap_err(), LOCK_ERR);

    peer.set_paused(false);
    assert!(tick_until(&a, 10, |a| !a.status().killed), "released after the first handshake: {:?}", a.network_status());
    assert_eq!(a.list_torrents()[0].status, "metadata");
    assert_eq!(a.status().route, "up");
}

/// The VPN server going quiet for longer than the stale threshold (a dead server, or the machine
/// sleeping) holds every torrent and HTTP; a fresh handshake releases them.
#[test]
fn peer_dying_holds_torrents_then_recovers() {
    let tmp = tempfile::tempdir().unwrap();
    let peer = TestPeer::start();
    let timing = NetTiming { stale_after: Duration::from_secs(3), start_grace: Duration::from_secs(5) };
    let a = vpn_app(tmp.path(), &vpn_settings(&peer, &conf_text(&peer.client_config())), timing);
    a.add_magnet(&magnet(11), &tmp.path().join("dl").to_string_lossy(), None, false).unwrap();
    assert!(tick_until(&a, 10, |a| a.status().route == "up"));
    // A healthy tunnel re-keys before it goes stale, so it stays up past the threshold.
    std::thread::sleep(Duration::from_secs(4));
    a.tick(Local::now().naive_local());
    assert!(!a.status().killed && a.status().route == "up", "{:?}", a.network_status());
    assert_eq!(a.list_torrents()[0].status, "metadata");

    peer.set_paused(true);
    assert!(tick_until(&a, 10, |a| a.status().killed), "{:?}", a.network_status());
    assert_eq!(a.status().route, "stale");
    assert_eq!(a.list_torrents()[0].status, "queued");
    assert_eq!(a.check_update().unwrap_err(), LOCK_ERR);
    std::thread::sleep(Duration::from_secs(2)); // well past the threshold, like a short sleep

    peer.set_paused(false);
    assert!(tick_until(&a, 15, |a| !a.status().killed), "recovers on a fresh handshake: {:?}", a.network_status());
    assert_eq!(a.status().route, "up");
    assert_eq!(a.list_torrents()[0].status, "metadata");
}

/// F1 (lab): a one-way path failure. The VPN server keeps sending handshake initiations but
/// nothing from us reaches it. The lock must hold once the stale threshold passes and keep
/// holding for as long as the cut lasts (the initiations are no proof of a live tunnel); once the
/// path is whole again a real handshake releases the torrents.
#[test]
fn one_way_cut_holds_torrents_and_keeps_them_held() {
    let tmp = tempfile::tempdir().unwrap();
    let peer = TestPeer::start();
    let timing = NetTiming { stale_after: Duration::from_secs(3), start_grace: Duration::from_secs(5) };
    let a = vpn_app(tmp.path(), &vpn_settings(&peer, &conf_text(&peer.client_config())), timing);
    a.add_magnet(&magnet(16), &tmp.path().join("dl").to_string_lossy(), None, false).unwrap();
    assert!(tick_until(&a, 10, |a| a.status().route == "up"));

    peer.set_inbound_cut(true);
    assert!(tick_until(&a, 8, |a| a.status().killed), "{:?}", a.network_status());
    // Two more stale periods with initiations arriving every second: the lock never lets go.
    let end = std::time::Instant::now() + Duration::from_secs(6);
    while std::time::Instant::now() < end {
        a.tick(Local::now().naive_local());
        let st = a.status();
        assert!(st.killed && st.route == "stale", "lock released during the cut: {:?}", a.network_status());
        assert_eq!(a.list_torrents()[0].status, "queued");
        std::thread::sleep(Duration::from_millis(200));
    }
    assert_eq!(a.check_update().unwrap_err(), LOCK_ERR);

    peer.set_inbound_cut(false);
    assert!(tick_until(&a, 15, |a| !a.status().killed), "recovers once the path is whole: {:?}", a.network_status());
    assert_eq!(a.status().route, "up");
    assert_eq!(a.list_torrents()[0].status, "metadata");
}

/// An active profile that cannot be opened starts behind a blocked SOCKS port: the route is not
/// up, the lock holds, and the engine never connects anywhere directly.
#[test]
fn invalid_active_profile_starts_blocked_not_direct() {
    let tmp = tempfile::tempdir().unwrap();
    let peer = TestPeer::start();
    let a = vpn_app(tmp.path(), &vpn_settings(&peer, "this is not a wireguard config"), NetTiming::default());
    let target = TcpListener::bind("127.0.0.1:0").unwrap();
    target.set_nonblocking(true).unwrap();
    let id = a.add_magnet(&magnet(12), &tmp.path().join("dl").to_string_lossy(), None, false).unwrap();
    a.engine.add_peer(&id, target.local_addr().unwrap()).unwrap();
    std::thread::sleep(Duration::from_secs(3));
    assert!(target.accept().is_err(), "the engine connected directly");
    a.tick(Local::now().naive_local());
    let st = a.status();
    assert_eq!(st.route, "down");
    assert!(st.killed);
    assert!(st.warnings.iter().any(|w| w.contains("Test VPN")), "{:?}", st.warnings);
    assert_eq!(a.check_update().unwrap_err(), LOCK_ERR);
    assert_eq!(a.network_status().route, Route::Down);
}

/// Without a profile the manual SOCKS5 proxy is used as before.
#[test]
fn manual_proxy_still_works_without_a_profile() {
    let tmp = tempfile::tempdir().unwrap();
    let stub = Socks5Stub::start("127.0.0.1:0".parse().unwrap(), None);
    let proxy = Some(ProxySettings { host: "127.0.0.1".into(), port: stub.addr().port(), username: None, password: None });
    Settings { proxy, ..Default::default() }.save(tmp.path()).unwrap();
    let a = app(tmp.path());
    let id = a.add_magnet(&magnet(13), &tmp.path().join("dl").to_string_lossy(), None, false).unwrap();
    a.engine.add_peer(&id, "127.0.0.1:9".parse().unwrap()).unwrap();
    assert!(wait_until(Duration::from_secs(10), || stub.saw_port(9)), "peer connection should use the manual proxy");
    a.tick(Local::now().naive_local());
    let st = a.status();
    assert_eq!((st.route.as_str(), st.profile_name.as_deref(), st.killed), ("none", None, false));
}

/// A profile wins over the manual proxy; clearing the profile goes back to the manual proxy.
#[test]
fn profile_wins_over_manual_proxy() {
    let tmp = tempfile::tempdir().unwrap();
    let peer = TestPeer::start();
    let stub = Socks5Stub::start("127.0.0.1:0".parse().unwrap(), None);
    let mut s = vpn_settings(&peer, &conf_text(&peer.client_config()));
    s.proxy = Some(ProxySettings { host: "127.0.0.1".into(), port: stub.addr().port(), username: None, password: None });
    let a = vpn_app(tmp.path(), &s, NetTiming::default());
    assert!(wait_until(Duration::from_secs(10), || a.network_status().route == Route::Up));
    let id = a.add_magnet(&magnet(14), &tmp.path().join("dl").to_string_lossy(), None, false).unwrap();
    let before = a.network_status().tx_bytes;
    a.engine.add_peer(&id, ECHO.parse().unwrap()).unwrap();
    assert!(wait_until(Duration::from_secs(15), || a.network_status().tx_bytes > before));
    assert_eq!(stub.connect_count(), 0, "the manual proxy must not be used while a profile is active");
    // The manual proxy being down does not matter while the profile is active.
    stub.stop();
    a.probe_proxy();
    a.tick(Local::now().naive_local());
    assert!(!a.status().killed && !a.status().proxy_down);

    // Profile off: back to the manual proxy (a fresh one on another port).
    let stub2 = Socks5Stub::start("127.0.0.1:0".parse().unwrap(), None);
    let mut s = a.settings_for_ui();
    s.network.active_profile = None;
    s.proxy = Some(ProxySettings { host: "127.0.0.1".into(), port: stub2.addr().port(), username: None, password: None });
    a.set_settings(s).unwrap();
    a.probe_proxy();
    assert_eq!(a.status().route, "none");
    a.engine.add_peer(&id, "127.0.0.1:9".parse().unwrap()).unwrap();
    assert!(wait_until(Duration::from_secs(10), || stub2.saw_port(9)), "back on the manual proxy");
}

/// The Test button: a temporary tunnel for a saved (not active) profile reports the handshake.
/// The exit-IP lookup goes through that tunnel (the TestPeer's DNS sees it) and, with no real
/// internet behind the TestPeer, comes back empty instead of failing the test.
#[test]
fn test_profile_reports_handshake_through_a_temporary_tunnel() {
    let tmp = tempfile::tempdir().unwrap();
    let peer = TestPeer::start();
    let mut s = vpn_settings(&peer, &conf_text(&peer.client_config()));
    s.network.active_profile = None;
    let a = vpn_app(tmp.path(), &s, NetTiming::default());
    let r = a.test_profile("p1").unwrap();
    assert_eq!((r.route, r.profile.as_deref(), r.exit_ip.as_deref()), (Route::Up, Some("Test VPN"), None));
    assert!(r.handshake_age_secs.is_some());
    assert!(peer.dns_queries().iter().any(|q| q.contains("api.ipify.org")), "{:?}", peer.dns_queries());
    assert!(a.test_profile("nope").is_err());
    assert_eq!(a.status().route, "none", "testing does not activate the profile");
}

/// Two writers racing: one clears the active profile, one sets it. Whatever order their apply
/// steps run in, the tunnel state must end on what was saved last (an apply working from a stale
/// copy must never put the engine on a route the saved settings do not say). Ends with a final
/// save of each kind and checks the engine really uses that route.
#[test]
fn racing_settings_saves_end_on_the_last_saved_profile_state() {
    let tmp = tempfile::tempdir().unwrap();
    let peer = TestPeer::start();
    let stub = Socks5Stub::start("127.0.0.1:0".parse().unwrap(), None);
    let mut s = vpn_settings(&peer, &conf_text(&peer.client_config()));
    s.proxy = Some(ProxySettings { host: "127.0.0.1".into(), port: stub.addr().port(), username: None, password: None });
    let a = Arc::new(vpn_app(tmp.path(), &s, NetTiming::default()));
    let set_profile = |a: &AppState, on: bool| {
        let mut s = a.settings_for_ui();
        s.network.active_profile = on.then(|| "p1".to_string());
        a.set_settings(s).unwrap();
    };
    // The tunnel state matches what is saved: a route exactly when a profile is saved.
    let consistent = |a: &AppState| (a.status().route != "none") == a.get_settings().network.active_profile.is_some();
    for round in 0..5 {
        let writers: Vec<_> = [false, true]
            .into_iter()
            .map(|on| {
                let a = a.clone();
                std::thread::spawn(move || (0..20).for_each(|_| set_profile(&a, on)))
            })
            .collect();
        writers.into_iter().for_each(|w| w.join().unwrap());
        assert!(consistent(&a), "round {round}: saved {:?} but route {}", a.get_settings().network.active_profile, a.status().route);
    }

    // Final save with the profile: the engine uses the tunnel and the lock is armed.
    set_profile(&a, true);
    assert_eq!(a.network_status().profile.as_deref(), Some("Test VPN"));
    assert_ne!(a.status().route, "none");
    assert!(wait_until(Duration::from_secs(10), || a.network_status().route == Route::Up));
    let id = a.add_magnet(&magnet(15), &tmp.path().join("dl").to_string_lossy(), None, false).unwrap();
    let before = a.network_status().tx_bytes;
    a.engine.add_peer(&id, ECHO.parse().unwrap()).unwrap();
    assert!(wait_until(Duration::from_secs(15), || a.network_status().tx_bytes > before), "engine is not on the tunnel");

    // Final save without: back on the manual proxy, not on a stale tunnel.
    set_profile(&a, false);
    assert_eq!((a.status().route.as_str(), a.network_status().profile), ("none", None));
    a.engine.add_peer(&id, "127.0.0.1:9".parse().unwrap()).unwrap();
    assert!(wait_until(Duration::from_secs(10), || stub.saw_port(9)), "engine is not on the manual proxy");
}
