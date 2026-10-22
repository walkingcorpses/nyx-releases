//! Task 6 tests: pause/resume/remove, file priorities, sequential mode, rate limits,
//! save-folder errors and the detail view.

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use nyx_engine::testkit::{free_loopback_addr, make_torrent, make_torrent_files, wait_until};
use nyx_engine::{AddOptions, Engine, EngineConfig, TorrentId, TorrentStatus};

/// A fresh engine on its own loopback port and state folder.
fn engine(tmp: &Path, name: &str) -> (Engine, SocketAddr) {
    let addr = free_loopback_addr();
    (Engine::new(EngineConfig::for_test(addr, tmp.join(name))).unwrap(), addr)
}

/// Seeder seeding `bytes`, leecher with it added and limited to `rate` bytes/s (0 = none),
/// already pointed at the seeder. Returns (seeder, leecher, id, leecher save dir).
fn setup(tmp: &Path, bytes: &[u8], rate: u64) -> (Engine, Engine, TorrentId, std::path::PathBuf) {
    let (seeder, saddr) = engine(tmp, "seeder");
    let (leecher, _) = engine(tmp, "leecher");
    seeder.add_torrent_file(bytes, AddOptions::new(tmp.join("seed"))).unwrap();
    assert!(wait_until(Duration::from_secs(20), || seeder.snapshot()[0].progress >= 1.0));
    let dl = tmp.join("dl");
    let id = leecher.add_torrent_file(bytes, AddOptions::new(&dl)).unwrap();
    if rate > 0 {
        leecher.set_limits(None, rate, 0).unwrap();
    }
    leecher.add_peer(&id, saddr).unwrap();
    (seeder, leecher, id, dl)
}

/// Progress of the first torrent in a snapshot.
fn progress(e: &Engine) -> f32 {
    e.snapshot()[0].progress
}

/// After pause the byte counter stops moving.
#[test]
fn pause_stops_progress() {
    let tmp = tempfile::tempdir().unwrap();
    let bytes = make_torrent(&tmp.path().join("seed"), 2 * 1024 * 1024);
    let (_s, l, id, _) = setup(tmp.path(), &bytes, 200 * 1024);
    assert!(wait_until(Duration::from_secs(10), || progress(&l) > 0.0));
    l.pause(&id).unwrap();
    assert!(wait_until(Duration::from_secs(5), || l.snapshot()[0].status == TorrentStatus::Paused));
    std::thread::sleep(Duration::from_millis(500));
    let p = progress(&l);
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(progress(&l), p);
}

/// Resume after pause carries on to completion.
#[test]
fn resume_continues() {
    let tmp = tempfile::tempdir().unwrap();
    let bytes = make_torrent(&tmp.path().join("seed"), 512 * 1024);
    let (_s, l, id, _) = setup(tmp.path(), &bytes, 256 * 1024);
    l.pause(&id).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    l.resume(&id).unwrap();
    assert!(wait_until(Duration::from_secs(30), || progress(&l) >= 1.0));
}

/// Remove with delete_files takes the payload off disk and out of the snapshot.
#[test]
fn remove_with_delete_removes_files_from_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let bytes = make_torrent(&tmp.path().join("seed"), 128 * 1024);
    let (_s, l, id, dl) = setup(tmp.path(), &bytes, 0);
    assert!(wait_until(Duration::from_secs(20), || progress(&l) >= 1.0));
    assert!(dl.join("payload.bin").exists());
    l.remove(&id, true).unwrap();
    assert!(l.snapshot().is_empty());
    assert!(wait_until(Duration::from_secs(10), || !dl.join("payload.bin").exists()));
}

/// Priority 0 on a file means it is never written, while the others finish.
#[test]
fn file_priority_zero_skips_file() {
    let tmp = tempfile::tempdir().unwrap();
    let bytes = make_torrent_files(&tmp.path().join("seed"), &[128 * 1024, 128 * 1024]);
    let (seeder, saddr) = engine(tmp.path(), "seeder");
    let (leecher, _) = engine(tmp.path(), "leecher");
    seeder.add_torrent_file(&bytes, AddOptions::new(tmp.path().join("seed"))).unwrap();
    assert!(wait_until(Duration::from_secs(20), || seeder.snapshot()[0].progress >= 1.0));
    let mut opts = AddOptions::new(tmp.path().join("dl"));
    opts.paused = true;
    let id = leecher.add_torrent_file(&bytes, opts).unwrap();
    leecher.set_file_priorities(&id, &[0, 4]).unwrap();
    leecher.resume(&id).unwrap();
    leecher.add_peer(&id, saddr).unwrap();
    assert!(wait_until(Duration::from_secs(30), || {
        leecher.torrent_detail(&id).unwrap().files[1].progress >= 1.0
    }));
    let d = leecher.torrent_detail(&id).unwrap();
    assert_eq!(d.files[0].priority, 0);
    assert_eq!(d.files[0].progress, 0.0);
}

/// Sequential mode fetches the first piece before the last one.
#[test]
fn sequential_has_first_piece_before_last() {
    let tmp = tempfile::tempdir().unwrap();
    let bytes = make_torrent(&tmp.path().join("seed"), 2 * 1024 * 1024);
    let (seeder, saddr) = engine(tmp.path(), "seeder");
    let (leecher, _) = engine(tmp.path(), "leecher");
    seeder.add_torrent_file(&bytes, AddOptions::new(tmp.path().join("seed"))).unwrap();
    assert!(wait_until(Duration::from_secs(20), || seeder.snapshot()[0].progress >= 1.0));
    let mut opts = AddOptions::new(tmp.path().join("dl"));
    opts.sequential = true;
    let id = leecher.add_torrent_file(&bytes, opts).unwrap();
    leecher.set_limits(None, 400 * 1024, 0).unwrap();
    leecher.add_peer(&id, saddr).unwrap();
    // Wait for some progress, then check the shape while the download is still under way.
    assert!(wait_until(Duration::from_secs(15), || progress(&leecher) > 0.2));
    assert!(progress(&leecher) < 1.0);
    let have = leecher.have_pieces(&id).unwrap();
    assert!(have[0], "first piece missing");
    assert!(!have[have.len() - 1], "last piece already present");
}

/// A 100 KiB/s cap holds a 2 MiB download to well under 800 KiB in 5 s.
#[test]
fn download_limit_caps_rate() {
    let tmp = tempfile::tempdir().unwrap();
    let bytes = make_torrent(&tmp.path().join("seed"), 2 * 1024 * 1024);
    let (_s, l, _id, _) = setup(tmp.path(), &bytes, 100 * 1024);
    std::thread::sleep(Duration::from_secs(5));
    let got = progress(&l) as f64 * 2.0 * 1024.0 * 1024.0;
    assert!(got < 800.0 * 1024.0, "downloaded {got} bytes in 5 s");
    assert!(got > 0.0, "nothing downloaded at all");
}

/// A save path that is an existing file gives that torrent an Error status and the engine keeps working.
#[test]
fn unwritable_save_path_sets_error_status() {
    let tmp = tempfile::tempdir().unwrap();
    let (e, _) = engine(tmp.path(), "e");
    let bytes = make_torrent(&tmp.path().join("seed"), 64 * 1024);
    let blocker = tmp.path().join("iamafile");
    std::fs::write(&blocker, b"x").unwrap();
    let id = e.add_torrent_file(&bytes, AddOptions::new(&blocker)).unwrap();
    let snap = e.snapshot();
    assert_eq!(snap.len(), 1);
    match &snap[0].status {
        TorrentStatus::Error(m) => assert!(!m.is_empty()),
        other => panic!("expected Error, got {other:?}"),
    }
    // Still answers other calls, and a second, healthy torrent works.
    let other = make_torrent(&tmp.path().join("seed2"), 32 * 1024);
    e.add_torrent_file(&other, AddOptions::new(tmp.path().join("ok"))).unwrap();
    assert_eq!(e.snapshot().len(), 2);
    e.remove(&id, false).unwrap();
}

/// A save folder that does not exist yet is created.
#[test]
fn missing_save_folder_is_created() {
    let tmp = tempfile::tempdir().unwrap();
    let (e, _) = engine(tmp.path(), "e");
    let bytes = make_torrent(&tmp.path().join("seed"), 64 * 1024);
    let target = tmp.path().join("a").join("b").join("c");
    e.add_torrent_file(&bytes, AddOptions::new(&target)).unwrap();
    assert!(target.is_dir());
}

/// The detail view lists the files, the connected peer and the (absent) trackers.
#[test]
fn detail_lists_files_peers_trackers() {
    let tmp = tempfile::tempdir().unwrap();
    let bytes = make_torrent_files(&tmp.path().join("seed"), &[64 * 1024, 64 * 1024]);
    let (_s, l, id, _) = setup(tmp.path(), &bytes, 32 * 1024);
    assert!(wait_until(Duration::from_secs(10), || !l.torrent_detail(&id).unwrap().peers.is_empty()));
    let d = l.torrent_detail(&id).unwrap();
    assert_eq!(d.files.len(), 2);
    assert_eq!(d.files[0].size, 64 * 1024);
    assert!(!d.peers.is_empty());
    assert!(d.trackers.is_empty());
}
