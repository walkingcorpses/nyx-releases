//! Task 8 test: a restart restores torrents, their progress and their categories.

use std::time::Duration;

use nyx_engine::testkit::{free_loopback_addr, make_torrent, wait_until};
use nyx_engine::{AddOptions, Engine, EngineConfig};

/// Download about half, save state, drop the engine, start a new one on the same state
/// folder: the torrent is back with at least the same progress, and keeps its category.
#[test]
fn restart_restores_torrents_and_progress() {
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().join("leecher");
    let seeder_addr = free_loopback_addr();
    let seeder = Engine::new(EngineConfig::for_test(seeder_addr, tmp.path().join("seeder"))).unwrap();
    let seed_dir = tmp.path().join("seed");
    let bytes = make_torrent(&seed_dir, 2 * 1024 * 1024);
    seeder.add_torrent_file(&bytes, AddOptions::new(&seed_dir)).unwrap();
    assert!(wait_until(Duration::from_secs(20), || seeder.snapshot()[0].progress >= 1.0));

    let before;
    let id;
    {
        let leecher = Engine::new(EngineConfig::for_test(free_loopback_addr(), &state)).unwrap();
        let mut opts = AddOptions::new(tmp.path().join("dl"));
        opts.category = Some("movies".into());
        id = leecher.add_torrent_file(&bytes, opts).unwrap();
        leecher.set_limits(None, 400 * 1024, 0).unwrap();
        leecher.add_peer(&id, seeder_addr).unwrap();
        assert!(wait_until(Duration::from_secs(20), || leecher.snapshot()[0].progress >= 0.4));
        leecher.pause(&id).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        leecher.save_state().unwrap();
        before = leecher.snapshot()[0].progress;
        assert!(before < 1.0);
    }

    let again = Engine::new(EngineConfig::for_test(free_loopback_addr(), &state)).unwrap();
    // Libtorrent may re-verify the files first; progress reads 0 until that finishes.
    wait_until(Duration::from_secs(20), || again.snapshot().first().is_some_and(|s| s.progress >= before - 0.001));
    let snap = again.snapshot();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].id, id);
    assert!(snap[0].progress >= before - 0.001, "progress fell from {before} to {}", snap[0].progress);
    assert_eq!(snap[0].category.as_deref(), Some("movies"));
}
