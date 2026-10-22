//! One Tauri command per operation in the ipc catalogue (`filebridge_ipc::for_each_operation!`),
//! plus the window's own. They only forward: no business logic lives in the window (ADR 0008).

use filebridge_ipc::{ErrorBody, Operations};
use tauri::State;

use crate::service::Service;
use crate::tray::TrayMenu;

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
            tauri_specta::collect_commands![$($name,)* service_endpoint, service_replacing, set_tray_labels]
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
#[tauri::command]
#[specta::specta]
pub fn set_tray_labels(tray: State<'_, TrayMenu>, show: String, quit: String) -> Res<()> {
    tray.set_labels(&show, &quit).map_err(|e| ErrorBody {
        code: "internal".into(),
        message: e.to_string(),
    })
}
