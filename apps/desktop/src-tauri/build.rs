fn main() {
    tauri_build::build();
    #[cfg(target_os = "macos")]
    ios::link_plugin();
}

/// iOS: builds the app's Swift plugin (`ios/`, the twin of the Android one in `gen/android`) and
/// links it into the app, the way `tauri-plugin` does for a plugin crate. Only a Mac builds iOS.
#[cfg(target_os = "macos")]
mod ios {
    use std::path::{Path, PathBuf};

    pub fn link_plugin() {
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("ios") {
            return;
        }
        let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets it"));
        // The Tauri Swift package the plugin imports (`ios/Package.swift`: `../.tauri/tauri-api`).
        let api = std::env::var("DEP_TAURI_IOS_LIBRARY_PATH")
            .expect("tauri sets DEP_TAURI_IOS_LIBRARY_PATH for its dependents");
        let dest = manifest.join(".tauri/tauri-api");
        let _ = std::fs::remove_dir_all(&dest);
        copy_dir(Path::new(&api), &dest).expect("copy the Tauri Swift package");
        println!("cargo:rerun-if-changed=ios/Package.swift");
        println!("cargo:rerun-if-changed=ios/Sources");
        tauri_utils::build::link_apple_library("filebridge-native", manifest.join("ios"));
    }

    /// Copies `from` into `to`, leaving out Swift's build output.
    fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            let name = entry.file_name();
            if [".build", "Package.resolved", "Tests"]
                .iter()
                .any(|n| name == *n)
            {
                continue;
            }
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                copy_dir(&path, &to.join(&name))?;
            } else {
                std::fs::copy(&path, to.join(&name))?;
            }
        }
        Ok(())
    }
}
