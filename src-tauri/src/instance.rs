// instance.rs - handing magnet links and .torrent paths to the running Nyx. The single-instance
// plugin delivers a second launch's arguments to the first process; this file filters them,
// parks them for the UI and wakes the window.

use std::io::Write;
use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppState;

/// Keeps only arguments Nyx should act on: magnet links and .torrent file paths. Anything
/// else (flags, other files) is dropped so a launch argument can never do more than add a torrent.
pub fn filter_args(args: &[String]) -> Vec<String> {
    args.iter()
        .filter(|a| {
            let l = a.to_ascii_lowercase();
            l.starts_with("magnet:") || l.ends_with(".torrent")
        })
        .cloned()
        .collect()
}

/// Appends a line to the file named by `NYX_TEST_LOG`, if set. Lets the integration tests see
/// what the running instance received. Unset in normal use, so nothing is ever written.
fn test_log(line: &str) {
    if let Some(path) = std::env::var_os("NYX_TEST_LOG") {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{line}");
        }
    }
}

/// A second launch forwarded its arguments to this (running) instance: queue the ones that
/// matter for the UI, show the window and tell the UI to look. Works whether the window is
/// visible, minimized or hidden in the tray.
pub fn handle_args(app: &AppHandle, args: &[String]) {
    let wanted = filter_args(args);
    if wanted.is_empty() {
        // A plain second launch just brings the existing window up.
        show_main(app);
        return;
    }
    for a in &wanted {
        test_log(&format!("forwarded: {a}"));
    }
    if let Some(state) = app.try_state::<Arc<AppState>>() {
        state.push_pending(wanted);
    }
    show_main(app);
    let _ = app.emit("app://open", ());
}

/// Arguments of this process's own launch: parked for the UI. Returns true if there were any,
/// so startup can show the window even when "start minimized" is on.
pub fn queue_own_args(app: &AppHandle, args: &[String]) -> bool {
    let wanted = filter_args(args);
    for a in &wanted {
        test_log(&format!("own: {a}"));
    }
    let any = !wanted.is_empty();
    if let Some(state) = app.try_state::<Arc<AppState>>() {
        state.push_pending(wanted);
    }
    any
}

/// Shows, un-minimizes and focuses the main window.
pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Takes the parked arguments (used by the `take_pending_args` command).
pub fn drain(state: &AppState) -> Vec<String> {
    state.take_pending()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_keeps_magnets_and_torrents_only() {
        let args: Vec<String> =
            ["nyx.exe", "--minimized", "magnet:?xt=urn:btih:abc", "C:\\x\\a.TORRENT", "C:\\x\\a.txt", "MAGNET:?xt=1"]
                .iter()
                .map(|s| s.to_string())
                .collect();
        assert_eq!(
            filter_args(&args),
            vec!["magnet:?xt=urn:btih:abc".to_string(), "C:\\x\\a.TORRENT".to_string(), "MAGNET:?xt=1".to_string()]
        );
    }
}
