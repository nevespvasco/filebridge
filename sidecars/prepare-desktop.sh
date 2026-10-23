#!/usr/bin/env bash
# Puts the desktop app's sidecars where `tauri build` takes them from (`bundle.externalBin` in
# apps/desktop/src-tauri/tauri.bundle.conf.json), named the Tauri way:
#
#   sidecars/bin/filebridge-service-<target triple>[.exe]   built here, in release
#   sidecars/bin/filebridge-<target triple>[.exe]           the CLI (apps/cli), built here
#   sidecars/bin/filebridge-rclone-<target triple>[.exe]    the pinned rclone (sidecars/RCLONE_VERSION)
#   sidecars/bin/share/                                      the CLI's man pages and completions
#                                                            (the .deb and .rpm install them)
#
# rclone gets its own name so that the .deb's /usr/bin/filebridge-rclone never clashes with the
# distribution's rclone package or shadows the user's (filebridge_host::find_rclone looks for it).
#
# `pnpm bundle` in apps/desktop runs it first (beforeBuildCommand, through
# apps/desktop/scripts/prepare-sidecars.mjs; prepare-desktop.ps1 is the Windows twin). rclone
# comes from, in this order:
#
#   RCLONE_DOWNLOAD=1   the pinned release from https://downloads.rclone.org/<version>/, for the
#                       target, checked against that release's SHA256SUMS before anything runs
#                       it (and the PGP signature on SHA256SUMS when gpg has rclone's key; with
#                       RCLONE_REQUIRE_SIGNATURE=1 it must). Kept in sidecars/bin/download/.
#   RCLONE=/path        that file.
#   otherwise           `rclone` on PATH, else ~/.local/bin/rclone.
#
# A local rclone is used only when `rclone version` is the pinned one. Building for another
# target (TAURI_ENV_TARGET_TRIPLE differs from this machine): use RCLONE_DOWNLOAD=1, or set RCLONE
# to that target's rclone, whose version can't be checked here.
set -euo pipefail
# Failures inside $(…) stop the script too (download_rclone runs in one).
shopt -s inherit_errexit

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
version="$(tr -d '[:space:]' < "$here/RCLONE_VERSION")"
# rclone's release signing key (https://rclone.org/install/#linux-installation-from-precompiled-binary).
rclone_key="FBF737ECE9F8AB18604BD2AC93935E02FF3B54FA"

cargo_bin="${CARGO:-$(command -v cargo || echo "$HOME/.cargo/bin/cargo")}"
rustc_bin="$(dirname "$cargo_bin")/rustc"
[[ -x "$rustc_bin" ]] || rustc_bin="$(command -v rustc)"
host="$("$rustc_bin" -vV | sed -n 's/^host: //p')"
triple="${TAURI_ENV_TARGET_TRIPLE:-$host}"
ext=""
[[ "$triple" == *windows* ]] && ext=".exe"

# rclone's name for the target's platform, as in its release file names.
rclone_platform() {
  case "$1" in
    x86_64-*-linux-*) echo linux-amd64 ;;
    aarch64-*-linux-*) echo linux-arm64 ;;
    armv7-*-linux-*) echo linux-arm-v7 ;;
    i686-*-linux-*) echo linux-386 ;;
    x86_64-pc-windows-*) echo windows-amd64 ;;
    aarch64-pc-windows-*) echo windows-arm64 ;;
    i686-pc-windows-*) echo windows-386 ;;
    x86_64-apple-darwin) echo osx-amd64 ;;
    aarch64-apple-darwin) echo osx-arm64 ;;
    *) return 1 ;;
  esac
}

# --- filebridge-service and the CLI -------------------------------------------------------------

# `tauri build --debug` bundles a debug build; the service matches it.
profile="release"
[[ "${TAURI_ENV_DEBUG:-false}" == "true" ]] && profile="debug"
target_dir="${CARGO_TARGET_DIR:-$root/target}"
args=(build -p filebridge-service -p filebridge-cli --manifest-path "$root/Cargo.toml")
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
install -m 0755 "$built/filebridge$ext" "$out/filebridge-$triple$ext"
echo "sidecar: $out/filebridge-$triple$ext"

# Man pages and completions don't depend on the target: a CLI built for this machine writes them.
cli="$built/filebridge$ext"
if [[ "$triple" != "$host" ]]; then
  "$cargo_bin" build -p filebridge-cli --release --manifest-path "$root/Cargo.toml"
  cli="$target_dir/release/filebridge"
fi
share="$out/share"
rm -rf "$share"
mkdir -p "$share/man/man1" "$share/completions"
"$cli" man --dir "$share/man/man1"
gzip -n -9 "$share/man/man1"/*.1
"$cli" completions bash > "$share/completions/filebridge.bash"
"$cli" completions zsh > "$share/completions/_filebridge"
"$cli" completions fish > "$share/completions/filebridge.fish"
echo "man pages and completions: $share"

# --- rclone -------------------------------------------------------------------------------------

sha256() {
  if command -v sha256sum >/dev/null; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

# The SHA-256 SHA256SUMS ($1) gives for file $2, from its signed part only.
expected_sha256() {
  awk -v name="$2" '
    /^-----BEGIN PGP SIGNATURE-----/ { exit }
    signed && $2 == name { print $1; exit }
    /^$/ { signed = 1 }
  ' "$1"
}

# The PGP signature on SHA256SUMS ($1), by rclone's key. Fails on a bad signature; without gpg or
# the key it says so and goes on, unless RCLONE_REQUIRE_SIGNATURE=1.
check_signature() {
  local status
  if command -v gpg >/dev/null; then
    status="$(gpg --batch --status-fd 1 --verify "$1" 2>/dev/null || true)"
    if grep -q "^\[GNUPG:\] VALIDSIG $rclone_key " <<<"$status"; then
      echo "SHA256SUMS: good signature from rclone's key $rclone_key"
      return 0
    fi
    if grep -q "^\[GNUPG:\] \(BADSIG\|VALIDSIG\)" <<<"$status"; then
      echo "SHA256SUMS: the signature is bad or not rclone's ($rclone_key): refusing it" >&2
      return 1
    fi
  fi
  if [[ "${RCLONE_REQUIRE_SIGNATURE:-0}" == "1" ]]; then
    echo "SHA256SUMS: can't check its signature: needs gpg with rclone's key ($rclone_key)" >&2
    return 1
  fi
  echo "SHA256SUMS: signature not checked (gpg doesn't have rclone's key $rclone_key); the" \
    "SHA-256 below still is" >&2
}

# Downloads the pinned rclone for the target into sidecars/bin/download/<version>/ (or reuses
# it), checks it, and prints the path of the rclone binary taken out of it.
download_rclone() {
  local platform name dir zip sums want got member bin
  platform="$(rclone_platform "$triple")" || {
    echo "no rclone release is known for $triple: set RCLONE instead" >&2
    return 1
  }
  name="rclone-$version-$platform"
  dir="$out/download/$version"
  zip="$dir/$name.zip"
  sums="$dir/SHA256SUMS"
  mkdir -p "$dir"
  if [[ ! -f "$sums" ]]; then
    curl -fsSL --proto '=https' --tlsv1.2 -o "$sums.part" \
      "https://downloads.rclone.org/$version/SHA256SUMS"
    mv "$sums.part" "$sums"
  fi
  check_signature "$sums" >&2 || { rm -f "$sums"; return 1; }
  want="$(expected_sha256 "$sums" "$name.zip")"
  if [[ -z "$want" ]]; then
    echo "SHA256SUMS of rclone $version has no $name.zip" >&2
    return 1
  fi
  if [[ ! -f "$zip" ]]; then
    echo "downloading https://downloads.rclone.org/$version/$name.zip" >&2
    curl -fsSL --proto '=https' --tlsv1.2 -o "$zip.part" \
      "https://downloads.rclone.org/$version/$name.zip"
    mv "$zip.part" "$zip"
  fi
  got="$(sha256 "$zip")"
  if [[ "$got" != "$want" ]]; then
    rm -f "$zip"
    echo "$name.zip: SHA-256 $got, but SHA256SUMS says $want: deleted, not used" >&2
    return 1
  fi
  echo "$name.zip: SHA-256 matches SHA256SUMS ($want)" >&2
  # Only the one member is read out of the archive.
  member="$name/rclone$ext"
  bin="$dir/$platform/rclone$ext"
  mkdir -p "$dir/$platform"
  if command -v unzip >/dev/null; then
    unzip -p "$zip" "$member" > "$bin.part"
  else
    python3 -I -c 'import sys, zipfile; sys.stdout.buffer.write(zipfile.ZipFile(sys.argv[1]).read(sys.argv[2]))' \
      "$zip" "$member" > "$bin.part"
  fi
  [[ -s "$bin.part" ]] || { echo "$name.zip has no $member" >&2; return 1; }
  chmod 0755 "$bin.part"
  mv "$bin.part" "$bin"
  echo "$bin"
}

rclone=""
if [[ "${RCLONE_DOWNLOAD:-0}" == "1" ]]; then
  rclone="$(download_rclone)"
elif [[ -n "${RCLONE:-}" ]]; then
  rclone="$RCLONE"
elif command -v rclone >/dev/null; then
  rclone="$(command -v rclone)"
elif [[ -x "$HOME/.local/bin/rclone" ]]; then
  rclone="$HOME/.local/bin/rclone"
fi
if [[ -z "$rclone" || ! -f "$rclone" ]]; then
  echo "rclone $version not found: run with RCLONE_DOWNLOAD=1 to download it (checked against" \
    "its SHA256SUMS), or set RCLONE=/path/to/rclone (https://downloads.rclone.org/$version/)" >&2
  exit 1
fi
if [[ "$triple" == "$host" ]]; then
  found="$("$rclone" version 2>/dev/null | head -n1 | awk '{print $2}')"
  if [[ "$found" != "$version" ]]; then
    echo "$rclone is rclone ${found:-?}, not the pinned $version (sidecars/RCLONE_VERSION):" >&2
    echo "run with RCLONE_DOWNLOAD=1, or set RCLONE to rclone $version (https://downloads.rclone.org/$version/)" >&2
    exit 1
  fi
fi
install -m 0755 "$rclone" "$out/filebridge-rclone-$triple$ext"
echo "sidecar: $out/filebridge-rclone-$triple$ext ($version, from $rclone)"
