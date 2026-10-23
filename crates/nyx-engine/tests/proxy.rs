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
    assert!(f.lsd && f.upnp && f.natpmp);
    let stub = Socks5Stub::start("127.0.0.1:0".parse().unwrap(), None);
    e.set_proxy(Some(cfg(&stub, None))).unwrap();
    let f = e.discovery_flags();
    assert!(!f.lsd && !f.upnp && !f.natpmp);
    e.set_proxy(None).unwrap();
    let f = e.discovery_flags();
    assert!(f.lsd && f.upnp && f.natpmp);
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
