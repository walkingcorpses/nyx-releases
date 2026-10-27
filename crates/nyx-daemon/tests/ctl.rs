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
