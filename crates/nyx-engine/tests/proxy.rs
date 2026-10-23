//! Task 5 tests: SOCKS5 proxy routing and leak protection, against a local stub.

use std::net::{SocketAddr, TcpListener};
use std::path::Path;
use std::time::Duration;

use nyx_engine::testkit::socks5::Socks5Stub;
use nyx_engine::testkit::{free_loopback_addr, make_torrent, wait_until};
use nyx_engine::{AddOptions, Engine, EngineConfig, ProxyConfig, TorrentId};

/// A fresh engine on its own loopback port and state folder.
fn engine(tmp: &Path, name: &str) -> (Engine, SocketAddr) {
    let addr = free_loopback_addr();
    (Engine::new(EngineConfig::for_test(addr, tmp.join(name))).unwrap(), addr)
}

/// Proxy settings pointing at the stub, with optional credentials.
fn cfg(stub: &Socks5Stub, creds: Option<(&str, &str)>) -> ProxyConfig {
    ProxyConfig {
        host: stub.addr().ip().to_string(),
        port: stub.addr().port(),
        username: creds.map(|c| c.0.to_string()),
        password: creds.map(|c| c.1.to_string()),
    }
}

/// Seeder already seeding a 256 KiB torrent, leecher with it added. (seeder, addr, leecher, id)
fn pair(tmp: &Path) -> (Engine, SocketAddr, Engine, TorrentId) {
    let (seeder, saddr) = engine(tmp, "seeder");
    let (leecher, _) = engine(tmp, "leecher");
    let dir = tmp.join("seed");
    let bytes = make_torrent(&dir, 256 * 1024);
    seeder.add_torrent_file(&bytes, AddOptions::new(&dir)).unwrap();
    assert!(wait_until(Duration::from_secs(20), || seeder.snapshot()[0].progress >= 1.0));
    let id = leecher.add_torrent_file(&bytes, AddOptions::new(tmp.join("dl"))).unwrap();
    (seeder, saddr, leecher, id)
}

/// With the proxy up, the leecher reaches the seeder via a SOCKS5 CONNECT and finishes.
#[test]
fn proxy_up_routes_peer_traffic_through_proxy() {
    let tmp = tempfile::tempdir().unwrap();
    let (_seeder, saddr, leecher, id) = pair(tmp.path());
    let stub = Socks5Stub::start("127.0.0.1:0".parse().unwrap(), None);
    leecher.set_proxy(Some(cfg(&stub, None))).unwrap();
    leecher.add_peer(&id, saddr).unwrap();
    assert!(wait_until(Duration::from_secs(30), || leecher.snapshot()[0].progress >= 1.0));
    assert!(stub.saw_port(saddr.port()), "no CONNECT to the seeder: {}", stub.connect_count());
}

/// Proxy configured but down: the leecher must not fall back to a direct connection.
#[test]
fn proxy_down_never_connects_directly() {
    let tmp = tempfile::tempdir().unwrap();
    let (leecher, _) = engine(tmp.path(), "leecher");
    let dir = tmp.path().join("seed");
    let bytes = make_torrent(&dir, 64 * 1024);
    let id = leecher.add_torrent_file(&bytes, AddOptions::new(tmp.path().join("dl"))).unwrap();
    // A plain listener stands in for the seeder and counts direct connections.
    let fake = TcpListener::bind("127.0.0.1:0").unwrap();
    fake.set_nonblocking(true).unwrap();
    let fake_addr = fake.local_addr().unwrap();
    // Proxy: start, point the engine at it, then stop it.
    let stub = Socks5Stub::start("127.0.0.1:0".parse().unwrap(), None);
    leecher.set_proxy(Some(cfg(&stub, None))).unwrap();
    stub.stop();
    leecher.add_peer(&id, fake_addr).unwrap();
    let mut direct = 0;
    wait_until(Duration::from_secs(10), || {
        while fake.accept().is_ok() {
            direct += 1;
        }
        false
    });
    assert_eq!(direct, 0, "engine connected directly while the proxy was down");
}

/// A stub that demands user/pass lets the engine through only with credentials.
#[test]
fn proxy_with_credentials_authenticates() {
    let tmp = tempfile::tempdir().unwrap();
    let (_seeder, saddr, leecher, id) = pair(tmp.path());
    let stub = Socks5Stub::start("127.0.0.1:0".parse().unwrap(), Some(("nyx".into(), "secret".into())));
    leecher.set_proxy(Some(cfg(&stub, Some(("nyx", "secret"))))).unwrap();
    leecher.add_peer(&id, saddr).unwrap();
    assert!(wait_until(Duration::from_secs(30), || leecher.snapshot()[0].progress >= 1.0));
    assert!(stub.connect_count() >= 1);
}

/// While a proxy is active LSD/UPnP/NAT-PMP are off; removing it restores the config.
#[test]
fn proxy_disables_lsd_upnp_natpmp() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = free_loopback_addr();
    let mut c = EngineConfig::for_test(addr, tmp.path().join("e"));
    c.discovery = true;
    let e = Engine::new(c).unwrap();
    let f = e.discovery_flags();
    assert!(f.lsd && f.upnp && f.natpmp && f.dht);
    let stub = Socks5Stub::start("127.0.0.1:0".parse().unwrap(), None);
    e.set_proxy(Some(cfg(&stub, None))).unwrap();
    let f = e.discovery_flags();
    assert!(!f.lsd && !f.upnp && !f.natpmp && !f.dht, "DHT must be off while proxied: {f:?}");
    e.set_proxy(None).unwrap();
    let f = e.discovery_flags();
    assert!(f.lsd && f.upnp && f.natpmp && f.dht);
}

/// While a proxy is active the engine must not accept direct inbound connections (answering one
/// would reveal the real IP); without a proxy it listens normally again.
#[test]
fn proxy_active_refuses_direct_inbound() {
    let tmp = tempfile::tempdir().unwrap();
    let (e, addr) = engine(tmp.path(), "e");
    let reachable = || std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok();
    assert!(wait_until(Duration::from_secs(5), reachable), "engine should listen without a proxy");
    let stub = Socks5Stub::start("127.0.0.1:0".parse().unwrap(), None);
    e.set_proxy(Some(cfg(&stub, None))).unwrap();
    assert!(wait_until(Duration::from_secs(5), || !reachable()), "still accepting direct connections with a proxy set");
    e.set_proxy(None).unwrap();
    assert!(wait_until(Duration::from_secs(5), reachable), "listening did not come back after removing the proxy");
}

/// A fake UDP tracker on loopback that records the source address of every datagram it gets.
fn fake_udp_tracker() -> (std::net::UdpSocket, SocketAddr) {
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
    let addr = sock.local_addr().unwrap();
    (sock, addr)
}

/// Collects datagram sources seen by `sock` for `secs` seconds (or until `stop_after` arrive).
fn sources_seen(sock: &std::net::UdpSocket, secs: u64, stop_after: usize) -> Vec<SocketAddr> {
    let mut seen = Vec::new();
    let end = std::time::Instant::now() + Duration::from_secs(secs);
    let mut buf = [0u8; 2048];
    while std::time::Instant::now() < end && seen.len() < stop_after {
        if let Ok((_, from)) = sock.recv_from(&mut buf) {
            seen.push(from);
        }
    }
    seen
}

/// An announce to a UDP tracker goes through the proxy's UDP relay, never straight to the tracker.
#[test]
fn proxy_up_routes_udp_tracker_through_proxy() {
    let tmp = tempfile::tempdir().unwrap();
    let (e, _) = engine(tmp.path(), "e");
    let (tracker, taddr) = fake_udp_tracker();
    let stub = Socks5Stub::start("127.0.0.1:0".parse().unwrap(), None);
    e.set_proxy(Some(cfg(&stub, None))).unwrap();
    let magnet = format!("magnet:?xt=urn:btih:{}&tr=udp%3A%2F%2F{}%2Fannounce", "12".repeat(20), taddr);
    e.add_magnet(&magnet, AddOptions::new(tmp.path().join("dl"))).unwrap();
    let seen = sources_seen(&tracker, 20, 1);
    assert!(!seen.is_empty(), "tracker never heard an announce (udp relayed: {})", stub.udp_count());
    let relays = stub.udp_relay_addrs();
    assert!(seen.iter().all(|s| relays.contains(s)), "direct UDP reached the tracker: {seen:?} vs relays {relays:?}");
    assert!(stub.saw_udp_port(taddr.port()));
}

/// Proxy configured but down: no UDP announce reaches the tracker directly.
#[test]
fn proxy_down_sends_no_udp_directly() {
    let tmp = tempfile::tempdir().unwrap();
    let (e, _) = engine(tmp.path(), "e");
    let (tracker, taddr) = fake_udp_tracker();
    let stub = Socks5Stub::start("127.0.0.1:0".parse().unwrap(), None);
    e.set_proxy(Some(cfg(&stub, None))).unwrap();
    stub.stop();
    let magnet = format!("magnet:?xt=urn:btih:{}&tr=udp%3A%2F%2F{}%2Fannounce", "34".repeat(20), taddr);
    e.add_magnet(&magnet, AddOptions::new(tmp.path().join("dl"))).unwrap();
    let seen = sources_seen(&tracker, 8, 1);
    assert!(seen.is_empty(), "UDP leaked directly with the proxy down: {seen:?}");
}

/// A proxy given in the engine config is in force from the first packet: a UDP announce made right
/// after startup is relayed, never sent direct.
#[test]
fn proxy_from_config_applies_at_startup() {
    let tmp = tempfile::tempdir().unwrap();
    let (tracker, taddr) = fake_udp_tracker();
    let stub = Socks5Stub::start("127.0.0.1:0".parse().unwrap(), None);
    let mut c = EngineConfig::for_test(free_loopback_addr(), tmp.path().join("e"));
    c.proxy = Some(cfg(&stub, None));
    let e = Engine::new(c).unwrap();
    assert!(!e.discovery_flags().dht);
    let magnet = format!("magnet:?xt=urn:btih:{}&tr=udp%3A%2F%2F{}%2Fannounce", "56".repeat(20), taddr);
    e.add_magnet(&magnet, AddOptions::new(tmp.path().join("dl"))).unwrap();
    let seen = sources_seen(&tracker, 20, 1);
    assert!(!seen.is_empty(), "no announce reached the tracker");
    let relays = stub.udp_relay_addrs();
    assert!(seen.iter().all(|s| relays.contains(s)), "direct UDP: {seen:?} vs {relays:?}");
}
