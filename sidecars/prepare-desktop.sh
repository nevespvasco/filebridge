#!/usr/bin/env bash
# Puts the desktop app's sidecars where `tauri build` takes them from (`bundle.externalBin` in
# apps/desktop/src-tauri/tauri.bundle.conf.json), named the Tauri way:
#
#   sidecars/bin/filebridge-service-<target triple>[.exe]   built here, in release
#   sidecars/bin/filebridge-rclone-<target triple>[.exe]    the pinned rclone (sidecars/RCLONE_VERSION)
#
# rclone gets its own name so that the .deb's /usr/bin/filebridge-rclone never clashes with the
# distribution's rclone package or shadows the user's (filebridge_host::find_rclone looks for it).
#
# `pnpm bundle` in apps/desktop runs it first (beforeBuildCommand). rclone is copied, never
# downloaded: from RCLONE if set, else `rclone` on PATH, else ~/.local/bin/rclone, and only when
# `rclone version` is the pinned one. Get it from
# https://downloads.rclone.org/<version>/ (rclone-<version>-<os>-<arch>.zip).
#
# Building for another target (TAURI_ENV_TARGET_TRIPLE differs from this machine): set RCLONE
# to that target's rclone; its version can't be checked here.
# TODO: download the pinned rclone and check it against the release's SHA256SUMS, so a build
# machine needs no rclone of its own; a PowerShell twin for Windows builds.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
version="$(tr -d '[:space:]' < "$here/RCLONE_VERSION")"

cargo_bin="${CARGO:-$(command -v cargo || echo "$HOME/.cargo/bin/cargo")}"
rustc_bin="$(dirname "$cargo_bin")/rustc"
[[ -x "$rustc_bin" ]] || rustc_bin="$(command -v rustc)"
host="$("$rustc_bin" -vV | sed -n 's/^host: //p')"
triple="${TAURI_ENV_TARGET_TRIPLE:-$host}"
ext=""
[[ "$triple" == *windows* ]] && ext=".exe"

# `tauri build --debug` bundles a debug build; the service matches it.
profile="release"
[[ "${TAURI_ENV_DEBUG:-false}" == "true" ]] && profile="debug"
target_dir="${CARGO_TARGET_DIR:-$root/target}"
args=(build -p filebridge-service --manifest-path "$root/Cargo.toml")
[[ "$profile" == "release" ]] && args+=(--release)
built="$target_dir/$profile"
if [[ "$triple" != "$host" ]]; then
  args+=(--target "$triple")
  built="$target_dir/$triple/$profile"
fi
"$cargo_bin" "${args[@]}"

out="$here/bin"
mkdir -p "$out"
install -m 0755 "$built/filebridge-service$ext" "$out/filebridge-service-$triple$ext"
echo "sidecar: $out/filebridge-service-$triple$ext"

rclone=""
if [[ -n "${RCLONE:-}" ]]; then
  rclone="$RCLONE"
elif command -v rclone >/dev/null; then
  rclone="$(command -v rclone)"
elif [[ -x "$HOME/.local/bin/rclone" ]]; then
  rclone="$HOME/.local/bin/rclone"
fi
if [[ -z "$rclone" || ! -f "$rclone" ]]; then
  echo "rclone $version not found: set RCLONE=/path/to/rclone (https://downloads.rclone.org/$version/)" >&2
  exit 1
fi
if [[ "$triple" == "$host" ]]; then
  found="$("$rclone" version 2>/dev/null | head -n1 | awk '{print $2}')"
  if [[ "$found" != "$version" ]]; then
    echo "$rclone is rclone ${found:-?}, not the pinned $version (sidecars/RCLONE_VERSION):" >&2
    echo "set RCLONE to rclone $version (https://downloads.rclone.org/$version/)" >&2
    exit 1
  fi
fi
install -m 0755 "$rclone" "$out/filebridge-rclone-$triple$ext"
echo "sidecar: $out/filebridge-rclone-$triple$ext ($version, from $rclone)"
