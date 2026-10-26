#!/usr/bin/env bash
# Makes the iOS Xcode project (`gen/apple`, ADR 0034) and adds what Tauri's template can't be told
# through tauri.ios.conf.json: the system libraries the linked-in librclone (Go) needs, which a
# Rust staticlib can't pass on to Xcode's link. Run from apps/desktop on a Mac; CI runs it on every
# build (.github/workflows/ios.yml).
set -euo pipefail

pnpm tauri ios init --ci

project=src-tauri/gen/apple/project.yml
# Go's runtime and net package: CoreFoundation, and libresolv for DNS.
python3 - "$project" <<'PY'
import sys
path = sys.argv[1]
text = open(path).read()
anchor = "        ENABLE_BITCODE: false\n"
line = "        OTHER_LDFLAGS: $(inherited) -framework CoreFoundation -lresolv\n"
assert anchor in text, f"{path}: Tauri's template changed, update scripts/ios-project.sh"
if line not in text:
    text = text.replace(anchor, anchor + line, 1)
open(path, "w").write(text)
PY
(cd src-tauri/gen/apple && xcodegen generate --quiet)
