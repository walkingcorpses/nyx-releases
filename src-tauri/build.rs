//! Build script: generates the Tauri context and the per-command permissions. Listing the
//! commands in the app manifest means the UI can call ONLY these (see capabilities/default.json).

/// Every command the UI may invoke. Keep in sync with `commands.rs` and the capability file.
const COMMANDS: &[&str] = &[
    "list_torrents",
    "add_magnet",
    "add_torrent",
    "inspect_torrent",
    "torrent_action",
    "set_priorities",
    "set_torrent_options",
    "set_torrent_limits",
    "get_detail",
    "get_magnet",
    "move_in_queue",
    "get_settings",
    "set_settings",
    "register_associations",
    "unregister_associations",
    "take_pending_args",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("failed to run tauri-build");
}
