//! Task 3 tests: adding torrents, snapshots, duplicate handling and a real
//! download from a local seeder on loopback.

use std::time::Duration;

use nyx_core::ParseError;
use nyx_engine::testkit::{free_loopback_addr, make_torrent, wait_until};
use nyx_engine::{AddOptions, Engine, EngineConfig, EngineError, TorrentStatus};

/// A fresh engine on its own loopback port and state folder.
fn engine(tmp: &std::path::Path, name: &str) -> (Engine, std::net::SocketAddr) {
    let addr = free_loopback_addr();
    let e = Engine::new(EngineConfig::for_test(addr, tmp.join(name))).unwrap();
    (e, addr)
}

/// A magnet for an arbitrary info hash (nobody seeds it).
fn magnet(hash: &str) -> String {
    format!("magnet:?xt=urn:btih:{hash}&dn=ghost")
}

/// Adding a .torrent returns its lowercase hex info hash as the id.
#[test]
fn add_torrent_file_returns_info_hash_id() {
    let tmp = tempfile::tempdir().unwrap();
    let (e, _) = engine(tmp.path(), "a");
    let seed_dir = tmp.path().join("seed");
    let bytes = make_torrent(&seed_dir, 64 * 1024);
    let id = e.add_torrent_file(&bytes, AddOptions::new(&seed_dir)).unwrap();
    assert_eq!(id.len(), 40);
    assert!(id.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    assert_eq!(nyx_engine::inspect_torrent(&bytes).unwrap().id, id);
}

/// A magnet nobody serves sits in the metadata state.
#[test]
fn add_magnet_appears_in_snapshot_as_metadata() {
    let tmp = tempfile::tempdir().unwrap();
    let (e, _) = engine(tmp.path(), "a");
    let id = e.add_magnet(&magnet(&"ab".repeat(20)), AddOptions::new(tmp.path().join("dl"))).unwrap();
    let snap = e.snapshot();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].id, id);
    assert_eq!(snap[0].status, TorrentStatus::Metadata);
}

/// Junk input is rejected by validation before libtorrent sees it.
#[test]
fn invalid_magnet_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let (e, _) = engine(tmp.path(), "a");
    let err = e.add_magnet("magnet:?dn=nothing", AddOptions::new(tmp.path())).unwrap_err();
    assert_eq!(err, EngineError::Parse(ParseError::MissingHash));
    assert!(e.snapshot().is_empty());
}

/// Magnet then file for the same torrent is one torrent, no error.
#[test]
fn duplicate_add_returns_same_id() {
    let tmp = tempfile::tempdir().unwrap();
    let (e, _) = engine(tmp.path(), "a");
    let seed_dir = tmp.path().join("seed");
    let bytes = make_torrent(&seed_dir, 64 * 1024);
    let hash = nyx_engine::inspect_torrent(&bytes).unwrap().id;
    let a = e.add_magnet(&magnet(&hash), AddOptions::new(&seed_dir)).unwrap();
    let b = e.add_torrent_file(&bytes, AddOptions::new(&seed_dir)).unwrap();
    let c = e.add_torrent_file(&bytes, AddOptions::new(&seed_dir)).unwrap();
    assert_eq!(a, b);
    assert_eq!(b, c);
    assert_eq!(e.snapshot().len(), 1);
}

/// A leecher with one peer (the seeder, on loopback) finishes a 1 MiB download.
#[test]
fn download_completes_from_local_seeder() {
    let tmp = tempfile::tempdir().unwrap();
    let (seeder, seed_addr) = engine(tmp.path(), "seeder");
    let (leecher, _) = engine(tmp.path(), "leecher");
    let seed_dir = tmp.path().join("seed");
    let bytes = make_torrent(&seed_dir, 1024 * 1024);
    let sid = seeder.add_torrent_file(&bytes, AddOptions::new(&seed_dir)).unwrap();
    assert!(wait_until(Duration::from_secs(20), || seeder.snapshot()[0].status == TorrentStatus::Seeding));
    let lid = leecher.add_torrent_file(&bytes, AddOptions::new(tmp.path().join("dl"))).unwrap();
    assert_eq!(sid, lid);
    leecher.add_peer(&lid, seed_addr).unwrap();
    let done = wait_until(Duration::from_secs(30), || leecher.snapshot()[0].progress >= 1.0);
    assert!(done, "download did not finish: {:?}", leecher.snapshot());
}
