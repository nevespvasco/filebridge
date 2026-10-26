#!/usr/bin/env bash
# Cargo runner for `aarch64-apple-ios-sim` (CARGO_TARGET_AARCH64_APPLE_IOS_SIM_RUNNER): runs a test
# binary inside a booted iOS simulator, which reads the Mac's file system, so the path works as is.
# .github/workflows/ios.yml boots one and sets SIMULATOR_UDID.
set -euo pipefail
exec xcrun simctl spawn "${SIMULATOR_UDID:?boot a simulator and set SIMULATOR_UDID}" "$@"
