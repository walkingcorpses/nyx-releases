// commands.rs - the Tauri commands the UI can call. Each is a thin wrapper that forwards to
// AppState (state.rs), where the logic and tests live. The list here must match
// build.rs COMMANDS and capabilities/default.json, which is what limits the UI to these.

use std::sync::Arc;

use nyx_core::{Settings, TorrentId};
use tauri::State;

use nyx_app::dto::{
    AppUpdateDto, DetailDto, MetaDto, NetworkStatusDto, ProfileDto, RssReportDto, SearchResultDto, StatusDto, TorrentDto,
};
use nyx_app::state::AppState;
use crate::{assoc, instance};

type St<'a> = State<'a, Arc<AppState>>;

/// Every torrent right now: the UI's starting point before it follows `torrents://update`.
#[tauri::command]
pub fn list_torrents(state: St) -> Result<Vec<TorrentDto>, String> {
    Ok(state.list_torrents())
}

/// Adds a magnet link. `paused` lets the UI hold it while the user picks files.
#[tauri::command]
pub fn add_magnet(state: St, uri: String, save_path: String, category: Option<String>, paused: Option<bool>) -> Result<TorrentId, String> {
    state.add_magnet(&uri, &save_path, category, paused.unwrap_or(false))
}

/// Adds a .torrent from a file path.
#[tauri::command]
pub fn add_torrent(state: St, path: String, save_path: String, category: Option<String>, paused: Option<bool>) -> Result<TorrentId, String> {
    state.add_torrent(&path, &save_path, category, paused.unwrap_or(false))
}

/// Lists a .torrent's files for the add dialog, without adding it.
#[tauri::command]
pub fn inspect_torrent(state: St, path: String) -> Result<MetaDto, String> {
    state.inspect_torrent(&path)
}

/// pause, resume, remove, remove_delete, recheck, open_folder, sequential_on, sequential_off.
#[tauri::command]
pub fn torrent_action(state: St, id: String, action: String) -> Result<(), String> {
    state.torrent_action(&id, &action)
}

/// Per-file priorities for one torrent (0 = skip).
#[tauri::command]
pub fn set_priorities(state: St, id: String, priorities: Vec<u8>) -> Result<(), String> {
    state.set_priorities(&id, priorities)
}

/// Final folder and category for a torrent added from a magnet (chosen after the file list arrived).
#[tauri::command]
pub fn set_torrent_options(state: St, id: String, save_path: String, category: Option<String>) -> Result<(), String> {
    state.set_torrent_options(&id, &save_path, category)
}

/// Per-torrent speed caps in bytes per second (0 = unlimited).
#[tauri::command]
pub fn set_torrent_limits(state: St, id: String, down: u64, up: u64) -> Result<(), String> {
    state.set_torrent_limits(&id, down, up)
}

/// Files, peers and trackers for the detail drawer.
#[tauri::command]
pub fn get_detail(state: St, id: String) -> Result<DetailDto, String> {
    state.get_detail(&id)
}

/// A shareable magnet link for "copy magnet".
#[tauri::command]
pub fn get_magnet(state: St, id: String) -> Result<String, String> {
    state.get_magnet(&id)
}

/// Re-orders the queue.
#[tauri::command]
pub fn move_in_queue(state: St, id: String, position: u32) -> Result<(), String> {
    state.move_in_queue(&id, position);
    Ok(())
}

/// Current settings.
#[tauri::command]
pub fn get_settings(state: St) -> Result<Settings, String> {
    Ok(state.settings_for_ui())
}

/// Validates, saves and applies settings.
#[tauri::command]
pub fn set_settings(state: St, settings: Settings) -> Result<(), String> {
    state.set_settings(settings)
}

/// "Make Nyx the default": registers .torrent and magnet: for this exe (current user only).
#[tauri::command]
pub fn register_associations() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    assoc::register(&exe).map_err(|e| e.to_string())
}

/// Removes the registrations made by `register_associations`.
#[tauri::command]
pub fn unregister_associations() -> Result<(), String> {
    assoc::unregister().map_err(|e| e.to_string())
}

/// Magnets / .torrent paths that arrived (command line, second launch) since the last call.
#[tauri::command]
pub fn take_pending_args(state: St) -> Result<Vec<String>, String> {
    Ok(instance::drain(&state))
}

/// Proxy / kill-switch / update / startup-warning state for the banners.
#[tauri::command]
pub fn get_status(state: St) -> Result<StatusDto, String> {
    Ok(state.status())
}

/// The queue order, first to start first.
#[tauri::command]
pub fn get_queue_order(state: St) -> Result<Vec<TorrentId>, String> {
    Ok(state.queue_order())
}

/// Asks GitHub for a newer Nyx. Returns it, or null when up to date.
#[tauri::command]
pub fn check_update(state: St) -> Result<Option<AppUpdateDto>, String> {
    state.check_update()
}

/// Installs the update the last check found (download, checksum, swap). Takes effect on restart.
#[tauri::command]
pub fn install_update(state: St) -> Result<String, String> {
    state.install_update()
}

/// Starts the (updated) exe again and quits this one.
#[tauri::command]
pub fn restart_app(app: tauri::AppHandle) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // The new process waits for this one to exit before it starts (single-instance would turn it away).
    std::process::Command::new(exe)
        .env("NYX_WAIT_PID", std::process::id().to_string())
        .spawn()
        .map_err(|e| e.to_string())?;
    app.exit(0);
    Ok(())
}

/// Polls the RSS feeds now (same as the timer) and reports what happened.
#[tauri::command]
pub fn rss_run_now(state: St) -> Result<RssReportDto, String> {
    Ok(state.rss_run())
}

/// Searches the configured provider.
#[tauri::command]
pub fn search(state: St, query: String) -> Result<Vec<SearchResultDto>, String> {
    state.search(&query)
}

/// Downloads a .torrent from a search result link into a temp file; returns its path.
#[tauri::command]
pub fn fetch_torrent_url(state: St, url: String) -> Result<String, String> {
    state.fetch_torrent_url(&url)
}

/// Runs slow AppState work (DNS at import, a tunnel bring-up that waits for a handshake) on a
/// blocking thread so the window never freezes while it runs.
async fn blocking<T: Send + 'static>(
    state: &St<'_>,
    work: impl FnOnce(Arc<AppState>) -> T + Send + 'static,
) -> Result<T, String> {
    let s = Arc::clone(state.inner());
    tauri::async_runtime::spawn_blocking(move || work(s)).await.map_err(|e| e.to_string())
}

/// The saved VPN profiles (names, endpoints; never any key material).
#[tauri::command]
pub fn list_profiles(state: St) -> Result<Vec<ProfileDto>, String> {
    Ok(state.list_profiles())
}

/// Imports a WireGuard `.conf` file chosen in the file picker as a new profile.
#[tauri::command]
pub async fn import_profile(state: St<'_>, path: String, name: String, group: String) -> Result<ProfileDto, String> {
    blocking(&state, move |s| s.import_profile(&path, &name, &group)).await?
}

/// Renames a profile and changes its group label.
#[tauri::command]
pub fn rename_profile(state: St, id: String, name: String, group: String) -> Result<(), String> {
    state.rename_profile(&id, &name, &group)
}

/// Deletes a profile (refused while it is active or in the rotation pool).
#[tauri::command]
pub fn delete_profile(state: St, id: String) -> Result<(), String> {
    state.delete_profile(&id)
}

/// Brings a profile's tunnel up once to check the handshake and the exit IP. Can take ~35 s.
#[tauri::command]
pub async fn test_profile(state: St<'_>, id: String) -> Result<NetworkStatusDto, String> {
    blocking(&state, move |s| s.test_profile(&id)).await?
}

/// Live VPN route, handshake age, traffic and next rotation for the status card.
#[tauri::command]
pub fn get_network_status(state: St) -> Result<NetworkStatusDto, String> {
    Ok(state.network_status())
}

/// Switches to another pool profile now (same as the timer does). Waits up to ~15 s.
#[tauri::command]
pub async fn rotate_now(state: St<'_>) -> Result<String, String> {
    blocking(&state, |s| s.rotate_now()).await?
}
