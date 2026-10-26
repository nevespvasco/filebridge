//! rclone linked into the process (ADR 0014): phones can't start the `rclone rcd` sidecar, so
//! they call librclone's C `RcloneRPC` (`sidecars/librclone`), which takes the same rc methods
//! and JSON as the HTTP API. Android loads it as a shared library; iOS links it into the app
//! (feature `librclone-linked`), since Go builds only static archives for iOS.

use std::ffi::{CStr, CString, c_char, c_int};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use filebridge_transport_api::{Result, TransportError};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::client::{RcCall, answer};
use crate::fs::Scrubber;

#[repr(C)]
struct RpcResult {
    output: *mut c_char,
    status: c_int,
}

type InitFn = unsafe extern "C" fn();
type SetenvFn = unsafe extern "C" fn(*const c_char, *const c_char);
type RpcFn = unsafe extern "C" fn(*const c_char, *const c_char) -> RpcResult;
type FreeFn = unsafe extern "C" fn(*mut c_char);

/// librclone, loaded once per process and never unloaded: rclone keeps global state (config,
/// stats, jobs), so a second load of the same library would share it anyway.
pub struct Librclone {
    /// The file it was loaded from; `None` when it is linked into the program.
    path: Option<PathBuf>,
    rpc: RpcFn,
    free: FreeFn,
    /// Secrets in use with rclone, scrubbed from every error it returns.
    scrubber: Scrubber,
    _lib: Option<libloading::Library>,
}

static LOADED: Mutex<Option<Arc<Librclone>>> = Mutex::new(None);

/// What librclone is told about the host's environment before it starts.
const FORWARDED_ENV: &[&str] = &["HOME", "TMPDIR", "XDG_CONFIG_HOME", "XDG_CACHE_HOME"];

/// librclone's exports, from the static archive `build.rs` links (`sidecars/librclone/shim`).
#[cfg(feature = "librclone-linked")]
mod linked {
    use super::{RpcResult, c_char};

    unsafe extern "C" {
        pub fn RcloneInitialize();
        pub fn RcloneSetenv(key: *const c_char, value: *const c_char);
        pub fn RcloneRPC(method: *const c_char, input: *const c_char) -> RpcResult;
        pub fn RcloneFreeString(s: *mut c_char);
    }
}

impl Librclone {
    /// Loads librclone from `path` (a file, or a bare name such as `librclone.so` that the
    /// system loader finds: on Android, the app's own native libraries) and initialises it
    /// like the sidecar: no config file, names compared byte for byte.
    ///
    /// Later calls return the library already loaded; a different `path` is an error.
    pub async fn open(path: &Path) -> Result<Arc<Self>> {
        Self::start(Some(path), || Self::load(path)).await
    }

    /// The librclone linked into this program (iOS), initialised as [`Librclone::open`] does.
    #[cfg(feature = "librclone-linked")]
    pub async fn open_linked() -> Result<Arc<Self>> {
        Self::start(None, || {
            // SAFETY: the archive exports these C signatures (sidecars/librclone/shim), and a
            // linked library is never unloaded.
            unsafe {
                init(linked::RcloneInitialize, Some(linked::RcloneSetenv));
            }
            Ok(Self {
                path: None,
                rpc: linked::RcloneRPC,
                free: linked::RcloneFreeString,
                scrubber: Scrubber::default(),
                _lib: None,
            })
        })
        .await
    }

    async fn start(path: Option<&Path>, load: impl FnOnce() -> Result<Self>) -> Result<Arc<Self>> {
        let lib = {
            let mut loaded = LOADED.lock().unwrap();
            if let Some(lib) = loaded.as_ref() {
                if lib.path.as_deref() != path {
                    return Err(TransportError::fatal(format!(
                        "librclone is already loaded from {}",
                        lib.path
                            .as_deref()
                            .map_or("this program".into(), |p| p.display().to_string())
                    )));
                }
                return Ok(lib.clone());
            }
            let lib = Arc::new(load()?);
            *loaded = Some(lib.clone());
            lib
        };
        // `--config ""` and `--no-unicode-normalization`, as `Rcd::start` passes them.
        lib.call("config/setpath", json!({ "path": "" })).await?;
        lib.call(
            "options/set",
            json!({ "main": { "NoUnicodeNormalization": true } }),
        )
        .await?;
        Ok(lib)
    }

    fn load(path: &Path) -> Result<Self> {
        let fail = |e: libloading::Error| {
            TransportError::fatal(format!("cannot load {}: {e}", path.display()))
        };
        // SAFETY: librclone's exports have these C signatures (sidecars/librclone/shim); the
        // symbols are only used while `_lib` keeps the library loaded.
        unsafe {
            let lib = libloading::Library::new(path).map_err(fail)?;
            let initialize = *lib.get::<InitFn>(b"RcloneInitialize\0").map_err(fail)?;
            let rpc = *lib.get::<RpcFn>(b"RcloneRPC\0").map_err(fail)?;
            let free = *lib.get::<FreeFn>(b"RcloneFreeString\0").map_err(fail)?;
            // Only our own build of librclone (sidecars/librclone/shim) has this.
            let setenv = lib.get::<SetenvFn>(b"RcloneSetenv\0").ok().map(|f| *f);
            init(initialize, setenv);
            Ok(Self {
                path: Some(path.to_path_buf()),
                rpc,
                free,
                scrubber: Scrubber::default(),
                _lib: Some(lib),
            })
        }
    }
}

/// Starts rclone. Go only sees the environment the process started with; pass what the host set
/// since (on phones, HOME and TMPDIR in the app's folders).
///
/// # Safety
/// `initialize` and `setenv` are librclone's, and stay valid for the life of the process.
unsafe fn init(initialize: InitFn, setenv: Option<SetenvFn>) {
    if let Some(setenv) = setenv {
        for key in FORWARDED_ENV {
            if let Some(value) = std::env::var_os(key)
                && let Ok(value) = CString::new(value.into_encoded_bytes())
            {
                let key = CString::new(*key).expect("static name");
                unsafe { setenv(key.as_ptr(), value.as_ptr()) };
            }
        }
    }
    unsafe { initialize() };
}

/// One blocking `RcloneRPC`. The pointers stay valid: the library is never unloaded.
fn rpc(funcs: (RpcFn, FreeFn), method: &str, input: &str) -> Result<(StatusCode, String)> {
    let (rpc, free) = funcs;
    let method_c = CString::new(method)
        .map_err(|_| TransportError::fatal(format!("rc method {method:?} has a NUL")))?;
    // serde_json escapes NUL, so the input never has one.
    let input_c = CString::new(input).expect("JSON has no NUL byte");
    // SAFETY: both strings outlive the call; rclone returns a malloc'd string that only
    // `RcloneFreeString` frees, once, after it is copied.
    let (output, status) = unsafe {
        let r = rpc(method_c.as_ptr(), input_c.as_ptr());
        let output = if r.output.is_null() {
            String::new()
        } else {
            let s = CStr::from_ptr(r.output).to_string_lossy().into_owned();
            free(r.output);
            s
        };
        (output, r.status)
    };
    let status = u16::try_from(status)
        .ok()
        .and_then(|s| StatusCode::from_u16(s).ok())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    Ok((status, output))
}

#[async_trait]
impl RcCall for Librclone {
    async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let funcs = (self.rpc, self.free);
        let owned = method.to_owned();
        let input = params.to_string();
        // A synchronous call (`operations/copyfile`) can take as long as the file: keep it off
        // the async threads.
        let (status, output) = tokio::task::spawn_blocking(move || rpc(funcs, &owned, &input))
            .await
            .map_err(|e| TransportError::fatal(format!("librclone task failed: {e}")))??;
        let body: Value = serde_json::from_str(&output).map_err(|e| {
            TransportError::retryable(format!("rclone rc {method}: bad response: {e}"))
        })?;
        answer(method, status, body, &self.scrubber)
    }

    fn scrubber(&self) -> &Scrubber {
        &self.scrubber
    }
}
