// lib.rs - Nyx app entry. Wires the Tauri shell to AppState: single instance, tray, the 1 Hz
// poller thread that feeds the UI, close-to-tray and saving state on exit.

pub mod assoc;
pub mod commands;
pub mod dto;
pub mod instance;
pub mod poller;
pub mod state;
pub mod tray;

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nyx_core::data_dir;
use tauri::{Emitter, Manager, PhysicalPosition, RunEvent, WindowEvent};
use tauri_plugin_notification::NotificationExt;

use dto::UpdateDto;
use state::AppState;

/// How often the poller saves resume data while running (ticks of one second).
const SAVE_EVERY_TICKS: u32 = 60;

/// Shows a blocking Windows message box. Used for startup failures, when no UI exists yet.
fn fatal(title: &str, text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    // SAFETY: both buffers are NUL-terminated and outlive the call; a null owner window is allowed.
    unsafe { MessageBoxW(std::ptr::null_mut(), wide(text).as_ptr(), wide(title).as_ptr(), MB_OK | MB_ICONERROR) };
}

/// True when the WebView2 runtime is installed (machine-wide or per user). Windows 11 ships it.
fn webview2_installed() -> bool {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use winreg::RegKey;
    const KEY: &str = "Software\\Microsoft\\EdgeUpdate\\Clients\\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
    let machine = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(format!("SOFTWARE\\WOW6432Node\\{}", &KEY[9..]));
    let user = RegKey::predef(HKEY_CURRENT_USER).open_subkey(KEY);
    [machine, user]
        .into_iter()
        .flatten()
        .any(|k| k.get_value::<String, _>("pv").map(|v| !v.is_empty() && v != "0.0.0.0").unwrap_or(false))
}

/// Where data lives: `NYX_DATA_DIR` (tests), else beside the exe if `nyx.portable` is there,
/// else `%APPDATA%\Nyx`.
fn resolve_data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("NYX_DATA_DIR") {
        return PathBuf::from(d);
    }
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())).unwrap_or_default();
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| exe_dir.clone());
    data_dir(&exe_dir, &appdata)
}

/// Starts Nyx. Returns when the app exits.
pub fn run() {
    if !webview2_installed() {
        fatal(
            "Nyx",
            "Nyx needs the Microsoft Edge WebView2 Runtime, which was not found.\n\nIt ships with Windows 11; on other systems install it from microsoft.com/edge/webview2.",
        );
        return;
    }
    let data = resolve_data_dir();
    let mut ctx = tauri::generate_context!();
    if std::env::var_os("NYX_DATA_DIR").is_some() {
        // A test instance with its own data folder must not single-instance into the user's real
        // Nyx: give it an identifier derived from the folder.
        let mut h = DefaultHasher::new();
        data.hash(&mut h);
        ctx.config_mut().identifier = format!("dev.nyx.client.t{:x}", h.finish());
    }

    let built = tauri::Builder::default()
        // Must come first: a second launch hands its arguments over and exits right here.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| instance::handle_args(app, &argv)))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            commands::list_torrents,
            commands::add_magnet,
            commands::add_torrent,
            commands::inspect_torrent,
            commands::torrent_action,
            commands::set_priorities,
            commands::set_torrent_options,
            commands::get_detail,
            commands::get_magnet,
            commands::move_in_queue,
            commands::get_settings,
            commands::set_settings,
            commands::register_associations,
            commands::unregister_associations,
            commands::take_pending_args,
        ])
        .on_window_event(|window, event| {
            // Close button: hide to the tray when that setting is on, otherwise let the app exit.
            if let WindowEvent::CloseRequested { api, .. } = event {
                let to_tray = window
                    .app_handle()
                    .try_state::<Arc<AppState>>()
                    .map(|s| s.get_settings().minimize_to_tray)
                    .unwrap_or(false);
                if to_tray {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .setup(move |app| {
            let listen: SocketAddr = std::env::var("NYX_LISTEN")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| "0.0.0.0:6881".parse().unwrap());
            let discovery = std::env::var_os("NYX_NO_DISCOVERY").is_none();
            let state = Arc::new(AppState::new(&data, listen, discovery)?);
            app.manage(state.clone());

            let args: Vec<String> = std::env::args().collect();
            let has_args = instance::queue_own_args(app.handle(), &args);
            tray::build(app.handle())?;
            spawn_poller(app.handle().clone(), state.clone());

            let minimized = state.get_settings().start_minimized || args.iter().any(|a| a == "--minimized");
            if let Some(w) = app.get_webview_window("main") {
                if std::env::var_os("NYX_TEST_OFFSCREEN").is_some() {
                    // Test runs open far off-screen so they never disturb whoever is at the PC.
                    let _ = w.set_position(PhysicalPosition::new(-32000, -32000));
                    let _ = w.set_skip_taskbar(true);
                }
                if !minimized || has_args {
                    let _ = w.show();
                }
            }
            Ok(())
        })
        .build(ctx);

    match built {
        Ok(app) => app.run(|app, event| {
            // Save resume data on the way out so the next start continues where this one stopped.
            if let RunEvent::Exit = event {
                if let Some(s) = app.try_state::<Arc<AppState>>() {
                    let _ = s.engine.save_state();
                }
            }
        }),
        Err(e) => fatal("Nyx could not start", &e.to_string()),
    }
}

/// Background thread: once a second, poll the engine, push changes to the UI, raise completion
/// notifications and (every minute) save resume data.
fn spawn_poller(app: tauri::AppHandle, state: Arc<AppState>) {
    std::thread::spawn(move || {
        let mut ticks = 0u32;
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let tick = state.tick(chrono::Local::now().naive_local());
            if !tick.update.is_empty() {
                let _ = app.emit("torrents://update", UpdateDto::from(&tick.update));
            }
            for name in tick.completed {
                let _ = app.notification().builder().title("Download complete").body(name).show();
            }
            ticks += 1;
            if ticks % SAVE_EVERY_TICKS == 0 {
                let _ = state.engine.save_state();
            }
        }
    });
}
