//! Tests for the daemon's control protocol. They call `handle` directly (no sockets) on an app state
//! bound to loopback with discovery off, so nothing here touches the network.

use nyx_app::state::AppState;
use nyx_daemon::ctl::handle;
use serde_json::{json, Value};

/// A fresh app state in `dir`: loopback port 0, no discovery.
fn app(dir: &std::path::Path) -> AppState {
    AppState::new(dir, "127.0.0.1:0".parse().unwrap(), false).unwrap()
}

/// Runs one request and parses the reply line.
fn call(state: &AppState, req: Value) -> Value {
    serde_json::from_str(&handle(state, &req.to_string())).expect("reply is JSON")
}

/// Settings read with `get_settings` can be written back with `set_settings` and read again unchanged.
#[test]
fn get_settings_roundtrips_through_set_settings() {
    let tmp = tempfile::tempdir().unwrap();
    let state = app(tmp.path());
    let before = call(&state, json!({"cmd": "get_settings"}));
    let mut s = before["ok"].clone();
    assert!(s.is_object(), "get_settings should reply ok: {before}");
    s["check_updates"] = json!(true);
    let set = call(&state, json!({"cmd": "set_settings", "settings": s}));
    assert!(set.get("ok").is_some(), "set_settings failed: {set}");
    let after = call(&state, json!({"cmd": "get_settings"}));
    assert_eq!(after["ok"]["check_updates"], json!(true));
    assert_eq!(after["ok"], s);
}

/// A command nobody defined is an error reply that names it.
#[test]
fn unknown_command_returns_err() {
    let tmp = tempfile::tempdir().unwrap();
    let r = call(&app(tmp.path()), json!({"cmd": "nope"}));
    assert!(r["err"].as_str().unwrap().contains("nope"), "{r}");
}

/// Garbage input must give an `err` reply, never a panic.
#[test]
fn malformed_json_returns_err_not_panic() {
    let tmp = tempfile::tempdir().unwrap();
    let state = app(tmp.path());
    for line in ["", "{", "not json", "[1,2]", "{\"cmd\": 5}", "{\"cmd\":\"set_settings\"}"] {
        let r: Value = serde_json::from_str(&handle(&state, line)).unwrap();
        assert!(r.get("err").is_some(), "line {line:?} gave {r}");
    }
}

/// A throwaway WireGuard config with an IP-literal endpoint (no DNS lookup at import).
const CONF: &str = "[Interface]\nPrivateKey = yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=\nAddress = 10.100.0.2/32\nDNS = 10.88.0.23\n\n[Peer]\nPublicKey = xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=\nAllowedIPs = 0.0.0.0/0, ::/0\nEndpoint = 10.88.0.11:51820\n";

/// import_profile stores a profile from a .conf path; list_profiles shows it (inactive, no keys).
#[test]
fn import_profile_then_list_profiles_shows_it() {
    let tmp = tempfile::tempdir().unwrap();
    let state = app(tmp.path());
    let path = tmp.path().join("b.conf");
    std::fs::write(&path, CONF).unwrap();
    let r = call(&state, json!({"cmd": "import_profile", "path": path.to_str().unwrap(), "name": "Exit B", "group": "lab"}));
    assert_eq!(r["ok"]["name"], json!("Exit B"), "{r}");
    let list = call(&state, json!({"cmd": "list_profiles"}));
    let profiles = list["ok"].as_array().expect("list_profiles replies a list");
    assert_eq!(profiles.len(), 1, "{list}");
    assert_eq!(profiles[0]["endpoint"], json!("10.88.0.11:51820"));
    assert_eq!(profiles[0]["active"], json!(false));
    assert!(!list.to_string().contains("PrivateKey"), "{list}");
}

/// import_profile without a path is an error naming the argument; a missing group is fine.
#[test]
fn import_profile_without_path_returns_err() {
    let tmp = tempfile::tempdir().unwrap();
    let r = call(&app(tmp.path()), json!({"cmd": "import_profile", "name": "x"}));
    assert!(r["err"].as_str().unwrap().contains("path"), "{r}");
}

/// The ctl client treats a connection closed without a reply as an error (it used to print
/// nothing and exit 0).
#[test]
fn ctl_request_errors_on_empty_reply() {
    let srv = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = srv.local_addr().unwrap().to_string();
    let t = std::thread::spawn(move || drop(srv.accept().unwrap()));
    let r = nyx_daemon::ctl::request(&addr, "{\"cmd\":\"status\"}", std::time::Duration::from_secs(5));
    t.join().unwrap();
    assert!(r.is_err(), "{r:?}");
}

/// The ctl client gives up after its timeout when the daemon accepts but never answers.
#[test]
fn ctl_request_times_out_on_silent_daemon() {
    let srv = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = srv.local_addr().unwrap().to_string();
    let t = std::thread::spawn(move || {
        let (_conn, _) = srv.accept().unwrap();
        std::thread::sleep(std::time::Duration::from_secs(2));
    });
    let r = nyx_daemon::ctl::request(&addr, "{\"cmd\":\"status\"}", std::time::Duration::from_millis(300));
    assert!(r.is_err(), "{r:?}");
    t.join().unwrap();
}

/// NYX_LISTEN: unset gives the default, a valid address is used, garbage is an error.
#[test]
fn listen_addr_rejects_garbage() {
    assert_eq!(nyx_daemon::listen_addr(None).unwrap().to_string(), "0.0.0.0:6881");
    assert_eq!(nyx_daemon::listen_addr(Some("10.0.0.1:7000")).unwrap().to_string(), "10.0.0.1:7000");
    assert!(nyx_daemon::listen_addr(Some("not-an-addr")).is_err());
    assert!(nyx_daemon::listen_addr(Some("0.0.0.0")).is_err());
}

/// A relative save folder is refused with the app's "full path" message.
#[test]
fn add_magnet_with_relative_save_path_returns_err() {
    let tmp = tempfile::tempdir().unwrap();
    let r = call(
        &app(tmp.path()),
        json!({"cmd": "add_magnet", "uri": "magnet:?xt=urn:btih:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "save_path": "relative/dir"}),
    );
    assert!(r["err"].as_str().unwrap().contains("full path"), "{r}");
}

/// network_status replies with the Network card's DTO: without a VPN profile the route reads
/// "down", there is no profile name and the byte counters are zero.
#[test]
fn network_status_without_profile_reports_down() {
    let tmp = tempfile::tempdir().unwrap();
    let r = call(&app(tmp.path()), json!({"cmd": "network_status"}));
    assert_eq!(r["ok"]["route"], json!("down"), "{r}");
    assert!(r["ok"]["profile"].is_null(), "{r}");
    assert_eq!(r["ok"]["rx_bytes"], json!(0), "{r}");
}

/// NYX_LAB_STALE_AFTER_SECS (lab-only): unset keeps the real thresholds; a number of seconds
/// replaces only the stale threshold; zero or garbage is a startup error.
#[test]
fn lab_timing_overrides_only_stale_after() {
    use nyx_app::netlock::NetTiming;
    use std::time::Duration;
    assert_eq!(nyx_daemon::lab_timing(None).unwrap(), NetTiming::default());
    let t = nyx_daemon::lab_timing(Some("20")).unwrap();
    assert_eq!(t.stale_after, Duration::from_secs(20));
    assert_eq!(t.start_grace, NetTiming::default().start_grace);
    assert!(nyx_daemon::lab_timing(Some("0")).is_err());
    assert!(nyx_daemon::lab_timing(Some("soon")).is_err());
}
