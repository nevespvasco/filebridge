//! What only a phone's OS gives (ADR 0013, ADR 0014), through the app's Android plugin
//! (`gen/android/.../FilebridgePlugin.kt`): a folder picked as a path, "All files access", the
//! foreground service that keeps a send alive, and secrets under an Android Keystore key.
//!
//! On desktop and iOS (no plugin yet) every call answers `unsupported` or does nothing, so the
//! commands, and the TypeScript bindings generated from them, are the same everywhere.

use filebridge_ipc::ErrorBody;
use serde::{Deserialize, Serialize};
use tauri::Manager;
use tauri::plugin::TauriPlugin;

/// The foreground notification while Jobs send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SendNotice {
    pub title: String,
    pub text: String,
    /// 0–100, or -1 when unknown.
    pub progress: i32,
}

/// Shared handle to the plugin, kept in Tauri's state.
#[derive(Clone)]
pub struct Native {
    #[cfg(target_os = "android")]
    handle: tauri::plugin::PluginHandle<tauri::Wry>,
}

#[cfg(target_os = "android")]
const ANDROID_PACKAGE: &str = "app.filebridge.desktop";

/// Registers the plugin and puts a [`Native`] in the app's state.
pub fn plugin() -> TauriPlugin<tauri::Wry> {
    tauri::plugin::Builder::new("filebridge")
        .setup(|app, _api| {
            #[cfg(target_os = "android")]
            let native = Native {
                handle: _api.register_android_plugin(ANDROID_PACKAGE, "FilebridgePlugin")?,
            };
            #[cfg(not(target_os = "android"))]
            let native = Native {};
            app.manage(native);
            Ok(())
        })
        .build()
}

/// What the plugin answers to `pickFolder`.
#[derive(Deserialize)]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct Picked {
    path: Option<String>,
}

#[derive(Deserialize)]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct Granted {
    granted: bool,
}

#[cfg(not(target_os = "android"))]
fn unsupported() -> ErrorBody {
    ErrorBody {
        code: "unsupported".into(),
        message: "only the Android app has this".into(),
    }
}

#[cfg(target_os = "android")]
fn plugin_error(e: tauri::plugin::mobile::PluginInvokeError) -> ErrorBody {
    use tauri::plugin::mobile::PluginInvokeError as E;
    match e {
        E::InvokeRejected(r) => ErrorBody {
            code: r.code.unwrap_or_else(|| "internal".into()),
            message: r.message.unwrap_or_default(),
        },
        other => ErrorBody {
            code: "internal".into(),
            message: other.to_string(),
        },
    }
}

impl Native {
    async fn call<T: serde::de::DeserializeOwned>(
        &self,
        command: &str,
        payload: impl Serialize,
    ) -> Result<T, ErrorBody> {
        #[cfg(target_os = "android")]
        {
            self.handle
                .run_mobile_plugin_async(command, payload)
                .await
                .map_err(plugin_error)
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = (command, serde_json::to_value(payload));
            Err(unsupported())
        }
    }

    /// The system folder picker; `None` when cancelled. Only folders on the phone's storage or
    /// an SD card have a path (code `not_local` otherwise).
    pub async fn pick_folder(&self) -> Result<Option<String>, ErrorBody> {
        let picked: Picked = self.call("pickFolder", ()).await?;
        Ok(picked.path)
    }

    /// Whether the app may read the phone's files ("All files access").
    pub async fn storage_access(&self) -> Result<bool, ErrorBody> {
        Ok(self.call::<Granted>("storageAccess", ()).await?.granted)
    }

    /// Opens Android's "All files access" screen for filebridge; answers once the user is back.
    pub async fn request_storage_access(&self) -> Result<bool, ErrorBody> {
        Ok(self
            .call::<Granted>("requestStorageAccess", ())
            .await?
            .granted)
    }

    /// Asks Android not to stop filebridge to save battery during long sends.
    pub async fn request_battery_exemption(&self) -> Result<(), ErrorBody> {
        self.call::<serde_json::Value>("requestBatteryExemption", ())
            .await
            .map(drop)
    }

    /// Starts or updates the foreground service with `notice`, or stops it (`None`).
    #[cfg_attr(not(mobile), allow(dead_code))]
    pub async fn sending(&self, notice: Option<&SendNotice>) {
        #[derive(Serialize)]
        struct Args<'a> {
            active: bool,
            #[serde(flatten)]
            notice: Option<&'a SendNotice>,
        }
        let args = Args {
            active: notice.is_some(),
            notice,
        };
        if let Err(e) = self.call::<serde_json::Value>("sending", args).await
            && e.code != "unsupported"
        {
            tracing::warn!("foreground service: {}", e.message);
        }
    }

    /// Where profiles' secrets go on this phone: Android has no keyring crate backend, so the
    /// plugin's Keystore vault; iOS uses the Keychain through the keyring crate (`None`).
    #[cfg(mobile)]
    pub fn secret_store(&self) -> Option<std::sync::Arc<dyn filebridge_secrets::SecretStore>> {
        #[cfg(target_os = "android")]
        {
            Some(std::sync::Arc::new(KeystoreStore {
                handle: self.handle.clone(),
            }))
        }
        #[cfg(not(target_os = "android"))]
        {
            None
        }
    }
}

/// Profiles' secrets in the plugin's vault: encrypted with a key that never leaves the Android
/// Keystore (ADR 0004). Calls block, as the trait expects; callers run them off the async threads.
#[cfg(target_os = "android")]
struct KeystoreStore {
    handle: tauri::plugin::PluginHandle<tauri::Wry>,
}

#[cfg(target_os = "android")]
impl filebridge_secrets::SecretStore for KeystoreStore {
    fn get(
        &self,
        r: &filebridge_secrets::KeyringRef,
    ) -> filebridge_secrets::Result<filebridge_secrets::Secret> {
        use filebridge_secrets::{Redacted, SecretError, Zeroizing};
        #[derive(Deserialize)]
        struct Value {
            value: Option<String>,
        }
        let got: Value = self
            .handle
            .run_mobile_plugin("secretGet", serde_json::json!({ "key": r.0 }))
            .map_err(keystore_error)?;
        got.value
            .map(|v| Redacted(Zeroizing::new(v)))
            .ok_or_else(|| SecretError::NotFound(r.0.clone()))
    }

    fn set(
        &self,
        r: &filebridge_secrets::KeyringRef,
        value: &str,
    ) -> filebridge_secrets::Result<()> {
        self.handle
            .run_mobile_plugin::<serde_json::Value>(
                "secretSet",
                serde_json::json!({ "key": r.0, "value": value }),
            )
            .map(drop)
            .map_err(keystore_error)
    }

    fn delete(&self, r: &filebridge_secrets::KeyringRef) -> filebridge_secrets::Result<()> {
        self.handle
            .run_mobile_plugin::<serde_json::Value>(
                "secretDelete",
                serde_json::json!({ "key": r.0 }),
            )
            .map(drop)
            .map_err(keystore_error)
    }
}

/// The plugin's error, which names the exception class only, never the value.
#[cfg(target_os = "android")]
fn keystore_error(e: tauri::plugin::mobile::PluginInvokeError) -> filebridge_secrets::SecretError {
    filebridge_secrets::SecretError::Unavailable(plugin_error(e).message)
}
