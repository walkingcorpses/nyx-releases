//! The daemon's control protocol: one JSON object per line in, one JSON object per line out.
//! Request `{"cmd": "...", ...args}`; reply `{"ok": <value>}` or `{"err": "<message>"}`.
//! `handle` is kept free of sockets so tests (and the TCP loop in main.rs) call the same code;
//! `request` is the client half used by `nyx-daemon ctl`.

use nyx_app::state::AppState;
use nyx_core::Settings;
use serde::Serialize;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

/// A string argument that must be present, or an error naming it.
fn arg_str<'a>(req: &'a Value, key: &str) -> Result<&'a str, String> {
    req.get(key).and_then(Value::as_str).ok_or_else(|| format!("missing string argument \"{key}\""))
}

/// Turns any serializable result into the `ok` value, so every command reports the same way.
fn ok<T: Serialize>(v: T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|e| e.to_string())
}

/// Runs one parsed request against the app. Errors are plain strings that become `{"err": ...}`.
fn dispatch(state: &AppState, req: &Value) -> Result<Value, String> {
    let cmd = req.get("cmd").and_then(Value::as_str).ok_or("missing string argument \"cmd\"")?;
    match cmd {
        "status" => ok(state.status()),
        "list" => ok(state.list_torrents()),
        "get_settings" => ok(state.settings_for_ui()),
        "set_settings" => {
            let s: Settings = serde_json::from_value(req.get("settings").cloned().ok_or("missing argument \"settings\"")?)
                .map_err(|e| format!("bad settings: {e}"))?;
            state.set_settings(s)?;
            Ok(json!(true))
        }
        "add_magnet" => {
            let paused = req.get("paused").and_then(Value::as_bool).unwrap_or(false);
            let category = req.get("category").and_then(Value::as_str).map(str::to_owned);
            ok(state.add_magnet(arg_str(req, "uri")?, arg_str(req, "save_path")?, category, paused)?)
        }
        "add_torrent" => {
            let paused = req.get("paused").and_then(Value::as_bool).unwrap_or(false);
            let category = req.get("category").and_then(Value::as_str).map(str::to_owned);
            ok(state.add_torrent(arg_str(req, "path")?, arg_str(req, "save_path")?, category, paused)?)
        }
        // Profiles: import seals the .conf at once; list never returns key material (get_settings
        // is redacted for the UI, so these are the lab's way to see and add profiles).
        "import_profile" => {
            let group = req.get("group").and_then(Value::as_str).unwrap_or("");
            ok(state.import_profile(arg_str(req, "path")?, arg_str(req, "name")?, group)?)
        }
        "list_profiles" => ok(state.list_profiles()),
        // The Network card: route (up/starting/stale/down), profile, handshake age, byte counters.
        "network_status" => ok(state.network_status()),
        // Rotate to another pool profile now (make-before-break); replies the new profile id.
        // Takes up to 15 s, inside the ctl client's 30 s timeout.
        "rotate_now" => ok(state.rotate_now()?),
        "rss_run" => ok(state.rss_run()),
        "check_update" => ok(state.check_update()?),
        "search" => ok(state.search(arg_str(req, "query")?)?),
        // Save resume data now; main.rs ends the process after sending this reply.
        "shutdown" => {
            state.engine.save_state().map_err(|e| e.to_string())?;
            Ok(json!("bye"))
        }
        other => Err(format!("unknown command \"{other}\"")),
    }
}

/// Request line to reply line. Never panics on bad input: unparseable JSON, a non-object or a
/// missing field all come back as `{"err": ...}`.
pub fn handle(state: &AppState, line: &str) -> String {
    let reply = match serde_json::from_str::<Value>(line) {
        Ok(req) => dispatch(state, &req).map(|v| json!({ "ok": v })),
        Err(e) => Err(format!("bad json: {e}")),
    };
    reply.unwrap_or_else(|e| json!({ "err": e })).to_string()
}

/// The `ctl` client side: sends one request line to `addr` and returns the reply line. An empty
/// reply (the daemon closed or died) is an error, and a daemon that never answers times out after
/// `timeout`, so a script driving the lab never hangs or mistakes silence for success.
pub fn request(addr: &str, json: &str, timeout: Duration) -> Result<String, String> {
    let mut conn = TcpStream::connect(addr).map_err(|e| format!("cannot reach daemon on {addr}: {e}"))?;
    conn.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    writeln!(conn, "{}", json.replace('\n', " ")).map_err(|e| e.to_string())?;
    let mut reply = String::new();
    BufReader::new(conn).read_line(&mut reply).map_err(|e| format!("no reply from daemon: {e}"))?;
    if reply.trim().is_empty() {
        return Err("the daemon closed the connection without a reply".into());
    }
    Ok(reply)
}

/// True when this request line is the `shutdown` command, so the server loop knows to exit after replying.
pub fn is_shutdown(line: &str) -> bool {
    serde_json::from_str::<Value>(line).ok().and_then(|v| v.get("cmd").and_then(Value::as_str).map(|c| c == "shutdown")).unwrap_or(false)
}
