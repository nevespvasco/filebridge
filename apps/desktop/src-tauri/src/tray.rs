//! Tray icon. Closing the window only hides it: sends keep running in the service, and the tray
//! brings the window back.

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

const SHOW: &str = "show";
const QUIT: &str = "quit";

/// The tray's menu items, so the UI can translate them.
pub struct TrayMenu {
    show: MenuItem<Wry>,
    quit: MenuItem<Wry>,
}

impl TrayMenu {
    pub fn set_labels(&self, show: &str, quit: &str) -> tauri::Result<()> {
        self.show.set_text(show)?;
        self.quit.set_text(quit)
    }
}

pub fn install(app: &AppHandle) -> tauri::Result<TrayMenu> {
    // Portuguese until the UI sends its translations.
    let show = MenuItem::with_id(app, SHOW, "Abrir filebridge", true, None::<&str>)?;
    let quit = MenuItem::with_id(
        app,
        QUIT,
        "Fechar janela (os envios continuam)",
        true,
        None::<&str>,
    )?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("filebridge")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            SHOW => show_main_window(app),
            // Quits the window process only; filebridge-service keeps sending.
            QUIT => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(TrayMenu { show, quit })
}

pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}
