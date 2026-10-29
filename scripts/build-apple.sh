#!/usr/bin/env bash
# Builds the Rust core for iOS and wraps it as an XCFramework for the Swift package in
# bindings/swift. Runs on a Mac with Xcode only.
#
#   scripts/build-apple.sh
#
# Leaves behind (not in git):
#   bindings/swift/build/UwUNewsCoreFFI.xcframework    what Package.swift points at
# and rewrites bindings/swift/Sources/UwUNewsCore/UwUNewsCore.swift, which is in git: commit it
# if it changed.
set -euo pipefail

cd "$(dirname "$0")/.."
root="$PWD"

fail() {
    echo "build-apple: $*" >&2
    exit 1
}

[ "$(uname -s)" = Darwin ] || fail "this needs a Mac"
command -v xcodebuild >/dev/null || fail "xcodebuild is missing: install Xcode"
command -v cargo >/dev/null ||
    fail "cargo is not on the PATH: with rustup from Homebrew, add \"\$(brew --prefix rustup)/bin\" to it"

# Package.swift and the app say the same.
export IPHONEOS_DEPLOYMENT_TARGET=26.0
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-3}"
export CARGO_INCREMENTAL=0

targets=(aarch64-apple-ios aarch64-apple-ios-sim)
# A simulator for Intel Macs only when asked: nobody here has one.
if [ "${UWUNEWS_X86_SIM:-0}" = 1 ]; then
    targets+=(x86_64-apple-ios)
fi
rustup target add "${targets[@]}" >/dev/null

target="${CARGO_TARGET_DIR:-$root/target}"
for triple in "${targets[@]}"; do
    cargo build -p uwunews-ffi --lib --profile ffi-release --target "$triple"
    [ -f "$target/$triple/ffi-release/libuwunews_ffi.a" ] || fail "no library for $triple"
done

"$root/scripts/bindings.sh"

out="$root/bindings/swift/build"
headers="$out/ffi"
universal="$out/universal"
rm -rf "$universal" "$out/UwUNewsCoreFFI.xcframework"
mkdir -p "$universal/simulator"

sims=("$target/aarch64-apple-ios-sim/ffi-release/libuwunews_ffi.a")
if [ "${UWUNEWS_X86_SIM:-0}" = 1 ]; then
    sims+=("$target/x86_64-apple-ios/ffi-release/libuwunews_ffi.a")
fi
lipo -create "${sims[@]}" -output "$universal/simulator/libuwunews_ffi.a"
xcodebuild -create-xcframework \
    -library "$target/aarch64-apple-ios/ffi-release/libuwunews_ffi.a" -headers "$headers" \
    -library "$universal/simulator/libuwunews_ffi.a" -headers "$headers" \
    -output "$out/UwUNewsCoreFFI.xcframework" >/dev/null
rm -rf "$universal"
echo "build-apple: $out/UwUNewsCoreFFI.xcframework"
