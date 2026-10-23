#!/usr/bin/env bash
# Builds the Android release APK, signed with your own key (docs/11-mobile.md).
#
#   apps/desktop/scripts/android-release.sh                 # arm64 phones and the x86_64 emulator
#   apps/desktop/scripts/android-release.sh aarch64         # phones only (smaller APK)
#
# One time, make the key. Keep the file and its password safe, outside the repo: every update
# must be signed with the same key, or Android refuses to install it over the old one, and
# uninstalling first deletes the app's data (its Jobs, Destinations and secrets).
#
#   keytool -genkeypair -v -keystore ~/.android/filebridge-release.jks -storetype PKCS12 \
#     -alias filebridge -keyalg RSA -keysize 4096 -validity 10000 -dname "CN=filebridge"
#
# Environment (Gradle reads the FILEBRIDGE_ANDROID_* ones, app/build.gradle.kts):
#   FILEBRIDGE_ANDROID_KEYSTORE           the keystore; default ~/.android/filebridge-release.jks
#                                         if it exists. With none, the debug key signs.
#   FILEBRIDGE_ANDROID_KEY_ALIAS          default filebridge
#   FILEBRIDGE_ANDROID_KEYSTORE_PASSWORD  asked without echo when unset
#   FILEBRIDGE_ANDROID_KEY_PASSWORD       default: the keystore's (a PKCS12 keystore has one)
#   ANDROID_HOME, NDK_HOME, JAVA_HOME     default ~/Android/Sdk, its newest NDK, a JDK 21 or 17
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
desktop="$(cd "$here/.." && pwd)"
root="$(cd "$desktop/../.." && pwd)"
android="$desktop/src-tauri/gen/android"

targets=("$@")
[[ ${#targets[@]} -gt 0 ]] || targets=(aarch64 x86_64)

export ANDROID_HOME="${ANDROID_HOME:-$HOME/Android/Sdk}"
export NDK_HOME="${NDK_HOME:-$(ls -d "$ANDROID_HOME"/ndk/* 2>/dev/null | sort -V | tail -1)}"
if [[ -z "${JAVA_HOME:-}" ]]; then
  for jdk in /usr/lib/jvm/java-21-openjdk-* /usr/lib/jvm/java-17-openjdk-*; do
    # A full JDK: Gradle needs javac, which a runtime-only package lacks.
    [[ -x "$jdk/bin/javac" ]] && { export JAVA_HOME="$jdk"; break; }
  done
fi
export PATH="$HOME/.cargo/bin:$PATH"
[[ -d "$ANDROID_HOME" ]] || { echo "Android SDK not found; set ANDROID_HOME" >&2; exit 1; }
[[ -d "$NDK_HOME" ]] || { echo "Android NDK not found; set NDK_HOME" >&2; exit 1; }
[[ -x "${JAVA_HOME:-}/bin/javac" ]] || { echo "JDK 17 or 21 (with javac) not found; set JAVA_HOME" >&2; exit 1; }

# librclone goes into the APK from jniLibs (ADR 0014); build it once, or after a shim change.
for t in "${targets[@]}"; do
  case "$t" in
    aarch64) abi=arm64-v8a ;;
    x86_64) abi=x86_64 ;;
    *) echo "unsupported target $t (librclone is built for aarch64 and x86_64)" >&2; exit 2 ;;
  esac
  if [[ ! -f "$android/app/src/main/jniLibs/$abi/librclone.so" ]]; then
    ANDROID_NDK_HOME="$NDK_HOME" "$root/sidecars/librclone/build.sh" android
    break
  fi
done

default_keystore="$HOME/.android/filebridge-release.jks"
if [[ -z "${FILEBRIDGE_ANDROID_KEYSTORE:-}" && -f "$default_keystore" ]]; then
  export FILEBRIDGE_ANDROID_KEYSTORE="$default_keystore"
fi
if [[ -n "${FILEBRIDGE_ANDROID_KEYSTORE:-}" ]]; then
  [[ -f "$FILEBRIDGE_ANDROID_KEYSTORE" ]] \
    || { echo "no keystore at $FILEBRIDGE_ANDROID_KEYSTORE" >&2; exit 1; }
  export FILEBRIDGE_ANDROID_KEY_ALIAS="${FILEBRIDGE_ANDROID_KEY_ALIAS:-filebridge}"
  if [[ -z "${FILEBRIDGE_ANDROID_KEYSTORE_PASSWORD:-}" ]]; then
    read -rsp "Password of $FILEBRIDGE_ANDROID_KEYSTORE: " FILEBRIDGE_ANDROID_KEYSTORE_PASSWORD </dev/tty
    echo
    export FILEBRIDGE_ANDROID_KEYSTORE_PASSWORD
  fi
  echo "signing with $FILEBRIDGE_ANDROID_KEYSTORE (alias $FILEBRIDGE_ANDROID_KEY_ALIAS)"
else
  echo "no keystore (see this script's header): signing with the Android debug key" >&2
fi

target_args=()
for t in "${targets[@]}"; do target_args+=(--target "$t"); done
(cd "$desktop" && pnpm tauri android build --apk "${target_args[@]}")

apk="$android/app/build/outputs/apk/universal/release/app-universal-release.apk"
[[ -f "$apk" ]] || { echo "no signed APK at $apk" >&2; exit 1; }
apksigner="$(ls -d "$ANDROID_HOME"/build-tools/*/apksigner 2>/dev/null | sort -V | tail -1 || true)"
if [[ -n "$apksigner" ]]; then
  "$apksigner" verify --print-certs "$apk" | grep -E "certificate (DN|SHA-256)"
fi
echo "APK: $apk"
