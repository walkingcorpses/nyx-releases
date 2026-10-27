//! Task 15 test: `rotate_identity` gives trackers a new peer_id and key and keeps every torrent
//! (progress, paused and queue-held flags, category, limits). A tiny HTTP tracker on loopback
//! records what each announce carries.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nyx_engine::testkit::{free_loopback_addr, make_torrent, make_torrent_tracked, wait_until};
use nyx_engine::{AddOptions, Engine, EngineConfig, TorrentStatus};

/// One announce as the tracker saw it (query values still percent-encoded: compared, not decoded).
#[derive(Clone, Debug)]
struct Announce {
    peer_id: String,
    key: String,
    event: String,
    user_agent: String,
}

/// HTTP tracker stub: answers every request with an empty peer list and records the announce.
struct TrackerStub {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<Announce>>>,
}

impl TrackerStub {
    /// Starts the stub on a free loopback port (its accept thread runs until the test ends).
    fn start() -> TrackerStub {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for mut c in l.incoming().flatten() {
                let mut req = Vec::new();
                let mut buf = [0u8; 2048];
                // Read the request head (libtorrent sends no body on a GET announce).
                while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                    match c.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => req.extend_from_slice(&buf[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&req).to_string();
                if let Some(a) = parse(&head) {
                    log.lock().unwrap().push(a);
                }
                let body = b"d8:intervali1800e5:peers0:e";
                let _ = write!(c, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = c.write_all(body);
            }
        });
        TrackerStub { addr, seen }
    }

    /// Every announce so far.
    fn announces(&self) -> Vec<Announce> {
        self.seen.lock().unwrap().clone()
    }
}

/// Pulls peer_id, key, event and the User-Agent out of an announce request head.
fn parse(head: &str) -> Option<Announce> {
    let line = head.lines().next()?;
    let query = line.split(' ').nth(1)?.split_once('?')?.1;
    let get = |name: &str| {
        query.split('&').find_map(|kv| kv.strip_prefix(name).and_then(|v| v.strip_prefix('='))).unwrap_or("").to_string()
    };
    let user_agent = head
        .lines()
        .find_map(|l| l.strip_prefix("User-Agent:").or_else(|| l.strip_prefix("user-agent:")))
        .unwrap_or("")
        .trim()
        .to_string();
    Some(Announce { peer_id: get("peer_id"), key: get("key"), event: get("event"), user_agent })
}

/// Three torrents (one announcing, one user-paused with a category and limits, one queue-held);
/// six rotations: after each, the announcing torrent comes back with a peer_id and a key never
/// seen before, the old identity never announces again (not even its "stopped"), nothing names
/// the client, and the other two keep their state.
#[test]
fn rotate_identity_keeps_torrents_and_changes_peer_id() {
    let tmp = tempfile::tempdir().unwrap();
    let tracker = TrackerStub::start();
    let url = format!("http://{}/announce", tracker.addr);
    let engine = Engine::new(EngineConfig::for_test(free_loopback_addr(), tmp.path().join("state"))).unwrap();
    let dl = |n: &str| tmp.path().join("dl").join(n);
    let a = engine.add_torrent_file(&make_torrent_tracked(&tmp.path().join("a"), 64 * 1024, &url), AddOptions::new(dl("a"))).unwrap();
    let mut opts = AddOptions::new(dl("b"));
    opts.paused = true;
    opts.category = Some("tv".into());
    let b = engine.add_torrent_file(&make_torrent(&tmp.path().join("b"), 32 * 1024), opts).unwrap();
    engine.set_limits(Some(&b), 50_000, 20_000).unwrap();
    let c = engine.add_torrent_file(&make_torrent(&tmp.path().join("c"), 48 * 1024), AddOptions::new(dl("c"))).unwrap();
    engine.set_queued(&c, true).unwrap();

    let started = |from: usize| {
        let t = &tracker;
        wait_until(Duration::from_secs(20), move || t.announces()[from..].iter().any(|x| x.event == "started"))
    };
    assert!(started(0), "no started announce: {:?}", tracker.announces());
    let first = tracker.announces().into_iter().find(|x| x.event == "started").unwrap();
    let mut ids: HashSet<String> = HashSet::from([first.peer_id.clone()]);
    let mut keys: HashSet<String> = HashSet::from([first.key.clone()]);
    let mut previous = first;
    for round in 1..=6 {
        let from = tracker.announces().len();
        engine.rotate_identity().unwrap();
        assert!(started(from), "round {round}: no announce after the rotation: {:?}", tracker.announces());
        std::thread::sleep(Duration::from_millis(500)); // a late announce of the old identity would land now
        let after = tracker.announces()[from..].to_vec();
        assert!(after.iter().all(|x| x.peer_id != previous.peer_id && x.key != previous.key),
                "round {round}: the old identity announced after the rotation: {after:?} (old {previous:?})");
        let now = after.iter().find(|x| x.event == "started").unwrap().clone();
        assert!(ids.insert(now.peer_id.clone()), "round {round}: peer_id {} seen before", now.peer_id);
        assert!(keys.insert(now.key.clone()), "round {round}: key {} seen before", now.key);
        previous = now;
    }
    for x in tracker.announces() {
        assert!(!x.peer_id.starts_with("-LT"), "peer_id names libtorrent: {x:?}");
        assert!(!x.user_agent.to_lowercase().contains("nyx"), "User-Agent names Nyx: {x:?}");
    }

    let snap = engine.snapshot();
    let got: HashSet<String> = snap.iter().map(|t| t.id.clone()).collect();
    assert_eq!(got, HashSet::from([a.clone(), b.clone(), c.clone()]));
    let row = |id: &str| snap.iter().find(|t| t.id == id).unwrap().clone();
    assert!(!matches!(row(&a).status, TorrentStatus::Paused | TorrentStatus::Queued), "a: {:?}", row(&a).status);
    assert_eq!(row(&b).status, TorrentStatus::Paused);
    assert_eq!(row(&b).category.as_deref(), Some("tv"));
    let lim = engine.torrent_detail(&b).unwrap();
    assert_eq!((lim.down_limit, lim.up_limit), (50_000, 20_000));
    assert_eq!(row(&c).status, TorrentStatus::Queued);
}
