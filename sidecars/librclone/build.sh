#!/usr/bin/env bash
# Builds rclone's librclone (ADR 0014): a shared library with a C `RcloneRPC(method, json)`, the
# same rc API the desktop reaches over HTTP.
#
#   sidecars/librclone/build.sh host      # sidecars/bin/librclone/host/librclone.so (tests)
#   sidecars/librclone/build.sh android   # .so per ABI into the Android app's jniLibs
#
# Needs Go (GO, default `go` on PATH, else ~/.local/go/bin/go) and, for android, the NDK
# (ANDROID_NDK_HOME, else the newest under ~/Android/Sdk/ndk).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"

# Same version as the desktop's rclone sidecar (sidecars/prepare-desktop.sh).
RCLONE_VERSION="$(tr -d '[:space:]' < "$root/sidecars/RCLONE_VERSION")"
# Lowest Android API the app supports (tauri.conf.json `bundle.android.minSdkVersion`).
ANDROID_API=26
src="$root/sidecars/bin/librclone/src-$RCLONE_VERSION"
jni="$root/apps/desktop/src-tauri/gen/android/app/src/main/jniLibs"

go_bin="${GO:-$(command -v go || echo "$HOME/.local/go/bin/go")}"
[[ -x "$go_bin" ]] || { echo "Go not found; set GO=/path/to/go" >&2; exit 1; }

if [[ ! -d "$src" ]]; then
  git clone --quiet --depth 1 --branch "$RCLONE_VERSION" https://github.com/rclone/rclone.git "$src"
fi

# Our shim (only the backends filebridge uses) inside rclone's module.
mkdir -p "$src/filebridge/quietconfig"
cp "$here/shim/main.go" "$src/filebridge/main.go"
cp "$here/shim/quietconfig/quietconfig.go" "$src/filebridge/quietconfig/quietconfig.go"

build() { # <out .so> [env…]
  local out="$1"; shift
  mkdir -p "$(dirname "$out")"
  (cd "$src" && env CGO_ENABLED=1 "$@" "$go_bin" build -trimpath -ldflags "-s -w" \
    -buildmode=c-shared -o "$out" ./filebridge)
  rm -f "${out%.so}.h"
  echo "built $out"
}

case "${1:-}" in
  host)
    build "$root/sidecars/bin/librclone/host/librclone.so"
    ;;
  android)
    # Android Studio's default SDK folder: ~/Android/Sdk on Linux, ~/Library/Android/sdk on macOS.
    ndk="${ANDROID_NDK_HOME:-${NDK_HOME:-$(ls -d "$HOME"/Android/Sdk/ndk/* "$HOME"/Library/Android/sdk/ndk/* 2>/dev/null | sort -V | tail -1)}}"
    [[ -d "$ndk" ]] || { echo "Android NDK not found; set ANDROID_NDK_HOME" >&2; exit 1; }
    # The NDK ships one prebuilt toolchain per host; macOS's is a universal binary named x86_64.
    case "$(uname -s)" in
      Darwin) host_tag=darwin-x86_64 ;;
      *) host_tag=linux-x86_64 ;;
    esac
    cc="$ndk/toolchains/llvm/prebuilt/$host_tag/bin"
    # ABI dir, GOARCH, clang target
    for t in "arm64-v8a arm64 aarch64-linux-android" "x86_64 amd64 x86_64-linux-android"; do
      read -r abi arch triple <<<"$t"
      build "$jni/$abi/librclone.so" GOOS=android GOARCH="$arch" CC="$cc/$triple$ANDROID_API-clang"
    done
    ;;
  *)
    echo "usage: $0 host|android" >&2; exit 2
    ;;
esac
