#!/usr/bin/env bash
# Builds rclone's librclone (ADR 0014): a shared library with a C `RcloneRPC(method, json)`, the
# same rc API the desktop reaches over HTTP.
#
#   sidecars/librclone/build.sh host          # sidecars/bin/librclone/host/librclone.so (tests)
#   sidecars/librclone/build.sh host-archive  # sidecars/bin/librclone/<host target>/librclone.a
#   sidecars/librclone/build.sh android       # .so per ABI into the Android app's jniLibs
#   sidecars/librclone/build.sh ios           # sidecars/bin/librclone/aarch64-apple-ios{,-sim}/librclone.a
#
# iOS gets a static archive (Go has no c-shared for iOS), which transport-rclone's
# `librclone-linked` feature links into the app; host-archive is the same for tests on Linux.
#
# Needs Go (GO, default `go` on PATH, else ~/.local/go/bin/go); for android the NDK
# (ANDROID_NDK_HOME, else the newest under ~/Android/Sdk/ndk); for ios a Mac with Xcode.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"

# Same version as the desktop's rclone sidecar (sidecars/prepare-desktop.sh).
RCLONE_VERSION="$(tr -d '[:space:]' < "$root/sidecars/RCLONE_VERSION")"
# Lowest Android API the app supports (tauri.conf.json `bundle.android.minSdkVersion`).
ANDROID_API=26
# Lowest iOS the app supports (tauri.conf.json `bundle.iOS.minimumSystemVersion`).
IOS_MIN=15.0
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

build() { # <out .so|.a> [env…]
  local out="$1"; shift
  local mode=c-shared
  [[ "$out" == *.a ]] && mode=c-archive
  mkdir -p "$(dirname "$out")"
  (cd "$src" && env CGO_ENABLED=1 "$@" "$go_bin" build -trimpath -ldflags "-s -w" \
    -buildmode="$mode" -o "$out" ./filebridge)
  rm -f "${out%.*}.h"
  echo "built $out"
}

case "${1:-}" in
  host)
    build "$root/sidecars/bin/librclone/host/librclone.so"
    ;;
  host-archive)
    rustc="$(command -v rustc || echo "$HOME/.cargo/bin/rustc")"
    target="$("$rustc" -vV | sed -n 's/^host: //p')"
    build "$root/sidecars/bin/librclone/$target/librclone.a"
    ;;
  ios)
    [[ "$(uname -s)" == Darwin ]] || { echo "ios needs a Mac with Xcode (CI: .github/workflows/ios.yml)" >&2; exit 1; }
    # Rust target, SDK, clang target. Go has one GOOS for both: the platform in the archive
    # comes from the C objects, so the simulator build gets the simulator's clang target.
    for t in "aarch64-apple-ios iphoneos arm64-apple-ios$IOS_MIN" \
             "aarch64-apple-ios-sim iphonesimulator arm64-apple-ios$IOS_MIN-simulator"; do
      read -r target sdk triple <<<"$t"
      sysroot="$(xcrun --sdk "$sdk" --show-sdk-path)"
      flags="-isysroot $sysroot -target $triple"
      build "$root/sidecars/bin/librclone/$target/librclone.a" GOOS=ios GOARCH=arm64 \
        CC="$(xcrun --sdk "$sdk" --find clang)" CGO_CFLAGS="$flags" CGO_LDFLAGS="$flags"
    done
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
    echo "usage: $0 host|host-archive|android|ios" >&2; exit 2
    ;;
esac
