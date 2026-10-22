//! rclone linked into the process (ADR 0014): phones can't start the `rclone rcd` sidecar, so
//! they load librclone (`sidecars/librclone`) and call its C `RcloneRPC`, which takes the same
//! rc methods and JSON as the HTTP API.

use std::ffi::{CStr, CString, c_char, c_int};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use filebridge_transport_api::{Result, TransportError};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::client::{RcCall, answer};

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
    path: PathBuf,
    rpc: RpcFn,
    free: FreeFn,
    _lib: libloading::Library,
}

static LOADED: Mutex<Option<Arc<Librclone>>> = Mutex::new(None);

/// What librclone is told about the host's environment before it starts.
const FORWARDED_ENV: &[&str] = &["HOME", "TMPDIR", "XDG_CONFIG_HOME", "XDG_CACHE_HOME"];

impl Librclone {
    /// Loads librclone from `path` (a file, or a bare name such as `librclone.so` that the
    /// system loader finds: on Android, the app's own native libraries) and initialises it
    /// like the sidecar: no config file, names compared byte for byte.
    ///
    /// Later calls return the library already loaded; a different `path` is an error.
    pub async fn open(path: &Path) -> Result<Arc<Self>> {
        let lib = {
            let mut loaded = LOADED.lock().unwrap();
            if let Some(lib) = loaded.as_ref() {
                if lib.path != path {
                    return Err(TransportError::fatal(format!(
                        "librclone is already loaded from {}",
                        lib.path.display()
                    )));
                }
                return Ok(lib.clone());
            }
            let lib = Arc::new(Self::load(path)?);
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
            let init = *lib.get::<InitFn>(b"RcloneInitialize\0").map_err(fail)?;
            let rpc = *lib.get::<RpcFn>(b"RcloneRPC\0").map_err(fail)?;
            let free = *lib.get::<FreeFn>(b"RcloneFreeString\0").map_err(fail)?;
            // Go only sees the environment the process started with; pass what the host set
            // since (on Android, HOME and TMPDIR in the app's folders). Only our own build of
            // librclone (sidecars/librclone/shim) has this.
            if let Ok(setenv) = lib.get::<SetenvFn>(b"RcloneSetenv\0") {
                for key in FORWARDED_ENV {
                    if let Some(value) = std::env::var_os(key)
                        && let Ok(value) = CString::new(value.into_encoded_bytes())
                    {
                        let key = CString::new(*key).expect("static name");
                        setenv(key.as_ptr(), value.as_ptr());
                    }
                }
            }
            init();
            Ok(Self {
                path: path.to_path_buf(),
                rpc,
                free,
                _lib: lib,
            })
        }
    }
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
        answer(method, status, body)
    }
}
