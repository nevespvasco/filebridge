//! With feature `librclone-linked`, links the static librclone that `sidecars/librclone/build.sh`
//! built for the target: `sidecars/bin/librclone/<target>/librclone.a`, or the folder in
//! `LIBRCLONE_LIB_DIR`.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=LIBRCLONE_LIB_DIR");
    if std::env::var_os("CARGO_FEATURE_LIBRCLONE_LINKED").is_none() {
        return;
    }
    let target = std::env::var("TARGET").expect("cargo sets TARGET");
    let dir = std::env::var_os("LIBRCLONE_LIB_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets it"))
                .join("../../sidecars/bin/librclone")
                .join(&target)
        });
    let archive = dir.join("librclone.a");
    assert!(
        archive.is_file(),
        "{} not found: build it with sidecars/librclone/build.sh, or set LIBRCLONE_LIB_DIR",
        archive.display()
    );
    println!("cargo:rerun-if-changed={}", archive.display());
    println!("cargo:rustc-link-search=native={}", dir.display());
    println!("cargo:rustc-link-lib=static=rclone");
    // What Go's runtime and net package need from the system.
    if target.contains("apple") {
        println!("cargo:rustc-link-lib=framework=CoreFoundation");
        println!("cargo:rustc-link-lib=framework=Security");
        println!("cargo:rustc-link-lib=resolv");
    } else {
        println!("cargo:rustc-link-lib=pthread");
        println!("cargo:rustc-link-lib=resolv");
    }
}
