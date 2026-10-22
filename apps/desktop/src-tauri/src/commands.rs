//! One Tauri command per operation in the ipc catalogue (`filebridge_ipc::for_each_operation!`),
//! plus the window's own. They only forward: no business logic lives in the window (ADR 0008).
//! On phones `Service` is the engine in the app (ADR 0014); the commands are the same.

use filebridge_ipc::{ErrorBody, Operations};
use serde::Serialize;
use tauri::State;

use crate::native::Native;
use crate::service::Service;

type Res<T> = Result<T, ErrorBody>;

/// One Tauri command per operation, forwarded to the service. Generated from the catalogue.
macro_rules! service_commands {
    ($(
        $(#[$doc:meta])*
        $variant:ident $name:ident ( $($arg:ident : $ty:ty),* ) -> $ret:ty;
    )*) => {
        $(
            $(#[$doc])*
            #[tauri::command]
            #[specta::specta]
            pub async fn $name(svc: State<'_, Service>, $($arg: $ty),*) -> Res<$ret> {
                svc.client().await?.$name($($arg),*).await
            }
        )*

        /// Every command: the service's, then the window's own.
        pub fn all() -> tauri_specta::Commands<tauri::Wry> {
            tauri_specta::collect_commands![
                $($name,)*
                service_endpoint,
                service_replacing,
                set_tray_labels,
                platform,
                pick_folder,
                storage_access,
                request_storage_access,
                request_battery_exemption,
                set_send_notice_labels,
            ]
        }
    };
}

filebridge_ipc::for_each_operation!(service_commands);

/// The socket path or pipe name the window connects to (for the Details panel).
#[tauri::command]
#[specta::specta]
pub fn service_endpoint(svc: State<'_, Service>) -> String {
    svc.endpoint()
}

/// Whether an older copy engine is being replaced right now (`ConnectionChanged` says when it
/// starts and ends; this covers one that started before the window listened).
#[tauri::command]
#[specta::specta]
pub fn service_replacing(svc: State<'_, Service>) -> bool {
    svc.replacing()
}

/// Translated tray menu labels; the strings live in the UI's i18n files.
#[cfg(desktop)]
#[tauri::command]
#[specta::specta]
pub fn set_tray_labels(
    tray: State<'_, crate::tray::TrayMenu>,
    show: String,
    quit: String,
) -> Res<()> {
    tray.set_labels(&show, &quit).map_err(|e| ErrorBody {
        code: "internal".into(),
        message: e.to_string(),
    })
}

/// Phones have no tray.
#[cfg(mobile)]
#[tauri::command]
#[specta::specta]
pub fn set_tray_labels(show: String, quit: String) -> Res<()> {
    let _ = (show, quit);
    Ok(())
}

/// Where the app runs, for what the UI shows (no tray, service or drag & drop on a phone).
#[derive(Debug, Clone, Copy, Serialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Desktop,
    Android,
    Ios,
}

#[tauri::command]
#[specta::specta]
pub fn platform() -> Platform {
    if cfg!(target_os = "android") {
        Platform::Android
    } else if cfg!(target_os = "ios") {
        Platform::Ios
    } else {
        Platform::Desktop
    }
}

/// Android: the system folder picker, as a path (`null` when cancelled).
#[tauri::command]
#[specta::specta]
pub async fn pick_folder(native: State<'_, Native>) -> Res<Option<String>> {
    native.pick_folder().await
}

/// Android: whether filebridge may read the phone's files ("All files access").
#[tauri::command]
#[specta::specta]
pub async fn storage_access(native: State<'_, Native>) -> Res<bool> {
    native.storage_access().await
}

/// Android: opens the "All files access" screen; answers whether it was granted.
#[tauri::command]
#[specta::specta]
pub async fn request_storage_access(native: State<'_, Native>) -> Res<bool> {
    native.request_storage_access().await
}

/// Android: asks not to be stopped to save battery during long sends.
#[tauri::command]
#[specta::specta]
pub async fn request_battery_exemption(native: State<'_, Native>) -> Res<()> {
    native.request_battery_exemption().await
}

/// Phones: the translated text of the notification shown while sending; `many` has `{n}`.
#[tauri::command]
#[specta::specta]
pub fn set_send_notice_labels(
    svc: State<'_, Service>,
    title: String,
    one: String,
    many: String,
) -> Res<()> {
    #[cfg(mobile)]
    svc.set_notice_labels(title, one, many);
    #[cfg(desktop)]
    let _ = (svc, title, one, many);
    Ok(())
}
