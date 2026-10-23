//! Task 4 tests: forced peer encryption refuses plaintext peers, accepts encrypted
//! ones, and "prefer" mode talks to plaintext peers.

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use nyx_engine::testkit::{free_loopback_addr, make_torrent, wait_until};
use nyx_engine::{AddOptions, Engine, EngineConfig, EncryptionMode, TorrentId};

/// A fresh engine on its own loopback port and state folder.
fn engine(tmp: &Path, name: &str) -> (Engine, SocketAddr) {
    let addr = free_loopback_addr();
    (Engine::new(EngineConfig::for_test(addr, tmp.join(name))).unwrap(), addr)
}

/// Seeder with a 256 KiB torrent already seeding, plus a leecher that has the torrent
/// added (not yet pointed at the seeder). Returns (seeder, seeder addr, leecher, id).
fn pair(tmp: &Path) -> (Engine, SocketAddr, Engine, TorrentId) {
    let (seeder, saddr) = engine(tmp, "seeder");
    let (leecher, _) = engine(tmp, "leecher");
    let seed_dir = tmp.join("seed");
    let bytes = make_torrent(&seed_dir, 256 * 1024);
    seeder.add_torrent_file(&bytes, AddOptions::new(&seed_dir)).unwrap();
    assert!(wait_until(Duration::from_secs(20), || seeder.snapshot()[0].progress >= 1.0));
    let id = leecher.add_torrent_file(&bytes, AddOptions::new(tmp.join("dl"))).unwrap();
    (seeder, saddr, leecher, id)
}

/// Engines start with encryption forced; a plaintext-only seeder proves it (see below).
/// Here we check the visible default through behaviour of a forced pair.
#[test]
fn default_mode_is_forced() {
    let tmp = tempfile::tempdir().unwrap();
    let (seeder, saddr, leecher, id) = pair(tmp.path());
    // Fresh engines, no set_encryption call: a plaintext-only seeder must be refused.
    seeder.set_plaintext_only().unwrap();
    leecher.add_peer(&id, saddr).unwrap();
    std::thread::sleep(Duration::from_secs(3));
    assert!(leecher.snapshot()[0].progress < 0.01);
}

/// Forced leecher + plaintext-only seeder: after 10 s nothing downloaded, no peers.
#[test]
fn forced_refuses_plaintext_peer() {
    let tmp = tempfile::tempdir().unwrap();
    let (seeder, saddr, leecher, id) = pair(tmp.path());
    seeder.set_plaintext_only().unwrap();
    leecher.set_encryption(EncryptionMode::Forced).unwrap();
    leecher.add_peer(&id, saddr).unwrap();
    std::thread::sleep(Duration::from_secs(10));
    let s = &leecher.snapshot()[0];
    assert_eq!(s.progress, 0.0);
    assert_eq!(s.peers, 0);
}

/// Forced on both ends: the download finishes and every peer shows as encrypted.
#[test]
fn forced_accepts_encrypted_peer() {
    let tmp = tempfile::tempdir().unwrap();
    let (_seeder, saddr, leecher, id) = pair(tmp.path());
    // Slow the download so the peer stays connected long enough to be sampled.
    leecher.set_limits(None, 32 * 1024, 0).unwrap();
    leecher.add_peer(&id, saddr).unwrap();
    let mut saw_peer = false;
    let done = wait_until(Duration::from_secs(60), || {
        // A peer still mid-handshake has no encryption flag yet. Once it shows interest
        // flags (I/i) the handshake is done, and only then must it read as encrypted.
        let d = leecher.torrent_detail(&id).unwrap();
        for p in d.peers.iter().filter(|p| p.flags.contains('I') || p.flags.contains('i')) {
            saw_peer = true;
            assert!(p.encrypted, "plaintext peer in forced mode: {p:?}");
        }
        leecher.snapshot()[0].progress >= 1.0
    });
    assert!(done);
    assert!(saw_peer, "never observed a connected peer");
}

/// Prefer mode on the leecher (plaintext allowed) connects to a plaintext-only seeder.
#[test]
fn prefer_mode_connects_to_plaintext_peer() {
    let tmp = tempfile::tempdir().unwrap();
    let (seeder, saddr, leecher, id) = pair(tmp.path());
    seeder.set_plaintext_only().unwrap();
    leecher.set_encryption(EncryptionMode::Prefer).unwrap();
    leecher.add_peer(&id, saddr).unwrap();
    assert!(wait_until(Duration::from_secs(60), || leecher.snapshot()[0].progress >= 1.0));
}
