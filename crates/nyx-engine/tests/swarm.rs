//! Real-swarm checks against the public internet. They are `#[ignore]`d so a normal test run
//! stays offline; run them with `cargo test -p nyx-engine --test swarm -- --ignored --nocapture`.
//! The torrent is "Big Buck Bunny" (a Blender Foundation film, CC-BY) from the WebTorrent project.

use std::time::{Duration, Instant};

use nyx_engine::testkit::socks5::Socks5Stub;
use nyx_engine::testkit::{free_loopback_addr, wait_until};
use nyx_engine::{AddOptions, Engine, EngineConfig, ProxyConfig, TorrentId, TorrentStatus};

const MAGNET: &str = "magnet:?xt=urn:btih:dd8255ecdc7ca55fb0bbf81323d87062db1f6d1c&dn=Big+Buck+Bunny\
&tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337&tr=udp%3A%2F%2Fexplodie.org%3A6969\
&tr=udp%3A%2F%2Ftracker.leechers-paradise.org%3A6969&tr=udp%3A%2F%2Fopen.stealth.si%3A80%2Fannounce\
&tr=wss%3A%2F%2Ftracker.btorrent.xyz&tr=wss%3A%2F%2Ftracker.openwebtorrent.com\
&ws=https%3A%2F%2Fwebtorrent.io%2Ftorrents%2F";

/// Downloads at least `target` bytes of the film, then reports the outcome. Skips every file but the
/// biggest one so we only pull a slice of data.
fn run(label: &str, mut cfg: EngineConfig, target: u64) -> (u64, Duration) {
    let tmp = tempfile::tempdir().unwrap();
    cfg.state_dir = tmp.path().join("state");
    let e = Engine::new(cfg).unwrap();
    let id: TorrentId = e.add_magnet(MAGNET, AddOptions::new(tmp.path().join("dl"))).unwrap();
    let t0 = Instant::now();
    assert!(
        wait_until(Duration::from_secs(120), || e.torrent_detail(&id).map(|d| !d.files.is_empty()).unwrap_or(false)),
        "{label}: no metadata after 120 s"
    );
    println!("{label}: metadata in {:.1}s", t0.elapsed().as_secs_f32());
    // Keep only the largest file.
    let files = e.torrent_detail(&id).unwrap().files;
    let big = files.iter().enumerate().max_by_key(|(_, f)| f.size).map(|(i, _)| i).unwrap();
    let prios: Vec<u8> = (0..files.len()).map(|i| if i == big { 4 } else { 0 }).collect();
    e.set_file_priorities(&id, &prios).unwrap();
    e.resume(&id).unwrap();
    let t1 = Instant::now();
    let got = |e: &Engine| (e.snapshot()[0].progress as f64 * e.snapshot()[0].size as f64) as u64;
    let ok = wait_until(Duration::from_secs(150), || {
        let s = &e.snapshot()[0];
        !matches!(s.status, TorrentStatus::Error(_)) && (s.progress as f64 * s.size as f64) as u64 >= target
    });
    let bytes = got(&e);
    println!("{label}: {} MiB in {:.1}s ({} peers)", bytes / (1024 * 1024), t1.elapsed().as_secs_f32(), e.snapshot()[0].peers);
    assert!(ok, "{label}: only {} MiB downloaded", bytes / (1024 * 1024));
    e.remove(&id, true).unwrap();
    (bytes, t1.elapsed())
}

/// Direct: DHT, trackers and peers on the real internet.
#[test]
#[ignore = "needs the internet"]
fn real_swarm_direct() {
    let mut cfg = EngineConfig::for_test("0.0.0.0:0".parse().unwrap(), "x");
    cfg.discovery = true;
    run("direct", cfg, 16 * 1024 * 1024);
}

/// Through a SOCKS5 proxy (our stub relays to the real internet): peers and trackers must still work,
/// and every peer connection must have gone through the proxy.
#[test]
#[ignore = "needs the internet"]
fn real_swarm_through_proxy() {
    let stub = Socks5Stub::start("127.0.0.1:0".parse().unwrap(), None);
    let mut cfg = EngineConfig::for_test(free_loopback_addr(), "x");
    cfg.discovery = true; // forced off by the proxy
    cfg.proxy = Some(ProxyConfig { host: "127.0.0.1".into(), port: stub.addr().port(), username: None, password: None });
    run("proxied", cfg, 4 * 1024 * 1024);
    println!("proxied: {} CONNECTs and {} UDP datagrams went through the proxy", stub.connect_count(), stub.udp_count());
    assert!(stub.connect_count() > 0, "no peer connection used the proxy");
}
