#!/usr/bin/env bash
# Generates the Swift source from the FFI crate:
#   bindings/swift/Sources/UwUNewsCore/UwUNewsCore.swift (in git)
#   bindings/swift/build/ffi/UwUNewsCoreFFI/ (header and module map, not in git)
#
# The generator reads the interface from a library built for this machine (a debug build).
set -euo pipefail

cd "$(dirname "$0")/.."
root="$PWD"

cargo build -p uwunews-ffi --features bindgen
target="${CARGO_TARGET_DIR:-$root/target}"
library="$target/debug/libuwunews_ffi.dylib"
[ -f "$library" ] || {
    echo "bindings: no library at $library" >&2
    exit 1
}

out="$root/bindings/swift/build/ffi"
rm -rf "$out"
(cd "$root/crates/uwunews-ffi" &&
    "$target/debug/uniffi-bindgen" generate "$library" --no-format --language swift --out-dir "$out")
mkdir -p "$root/bindings/swift/Sources/UwUNewsCore"
mv "$out/UwUNewsCore.swift" "$root/bindings/swift/Sources/UwUNewsCore/UwUNewsCore.swift"
# Xcode wants one directory per module with a `module.modulemap` in it.
mkdir -p "$out/UwUNewsCoreFFI"
mv "$out/UwUNewsCoreFFI.h" "$out/UwUNewsCoreFFI/UwUNewsCoreFFI.h"
mv "$out/UwUNewsCoreFFI.modulemap" "$out/UwUNewsCoreFFI/module.modulemap"
echo "bindings: Swift in bindings/swift/Sources/UwUNewsCore"
