//! `filebridge --version`: the crate version plus `git describe` of the checkout it was built
//! from (`0.0.1 (v0.1.0-3-gdb1a3ef)`, or the short commit while there are no tags). Built
//! outside a git checkout (a source tarball), it is the crate version alone.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let text = text.trim();
    (out.status.success() && !text.is_empty()).then(|| text.to_owned())
}

fn main() {
    println!("cargo:rerun-if-env-changed=FILEBRIDGE_GIT_DESCRIBE");
    // Packagers without the git history can set it themselves.
    let describe = std::env::var("FILEBRIDGE_GIT_DESCRIBE")
        .ok()
        .filter(|d| !d.is_empty())
        .or_else(|| git(&["describe", "--tags", "--always"]));
    // Described again when HEAD moves: a commit, a checkout, a new tag. Not on every edit, so
    // there is no `--dirty`: it would go stale between commits.
    let mut watch = vec!["HEAD".to_owned(), "packed-refs".to_owned()];
    if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
        watch.push(branch);
    }
    watch.push("refs/tags".to_owned());
    for path in watch {
        // Relative to this crate, as Cargo reads it. A missing path would rerun every build.
        let found = git(&["rev-parse", "--git-path", &path]);
        if let Some(p) = found.filter(|p| std::path::Path::new(p).exists()) {
            println!("cargo:rerun-if-changed={p}");
        }
    }
    let version = env!("CARGO_PKG_VERSION");
    let long = match describe {
        Some(d) => format!("{version} ({d})"),
        None => version.to_owned(),
    };
    println!("cargo:rustc-env=FILEBRIDGE_VERSION={long}");
}
