// tray.rs - the notification-area icon and its menu: Show, Pause all, Resume all, Quit.
// Left-click shows the window. Closing the window hides to the tray when that setting is on.

use std::sync::Arc;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

use crate::instance::show_main;
use crate::state::AppState;

/// Builds the tray icon and wires its menu.
pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show Nyx", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause_all", "Pause all", true, None::<&str>)?;
    let resume = MenuItem::with_id(app, "resume_all", "Resume all", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&show, &sep1, &pause, &resume, &sep2, &quit])?;

    let mut b = TrayIconBuilder::with_id("nyx")
        .tooltip("Nyx")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| {
            let state = app.try_state::<Arc<AppState>>();
            match ev.id.as_ref() {
                "show" => show_main(app),
                "pause_all" => state.iter().for_each(|s| s.pause_all()),
                "resume_all" => state.iter().for_each(|s| s.resume_all()),
                "quit" => app.exit(0),
                _ => {}
            }
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        b = b.icon(icon.clone());
    }
    b.build(app)?;
    Ok(())
}
