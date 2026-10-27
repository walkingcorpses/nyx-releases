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
