#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Builds the Rust core for the Mac app (architecture §16.1, step 1):
#
#   1. asiri-ffi for Apple silicon and Intel, combined into one static library,
#   2. the Swift bindings, C header and module map (UniFFI, library mode),
#   3. AsiriCoreFFI.xcframework, which the AsiriKit Swift package links.
#
#   tools/release/build-xcframework.sh [--debug] [--no-tor] [--host-only]
#
#   --debug      debug build (faster to compile, much larger)
#   --no-tor     leave out Arti: the app can then reach development relays only
#   --host-only  build for this Mac's architecture only
#
# Outputs (both ignored by git):
#   apps/macos/Frameworks/AsiriCoreFFI.xcframework
#   apps/macos/Packages/AsiriCoreBridge/Sources/AsiriCoreKit/Generated/AsiriCoreKit.swift
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"

profile=release
features=()
targets=(aarch64-apple-darwin x86_64-apple-darwin)
for arg in "$@"; do
  case "$arg" in
    --debug) profile=debug ;;
    --no-tor) features=(--no-default-features) ;;
    --host-only) targets=("$(rustc -vV | sed -n 's/^host: //p')") ;;
    -h|--help) sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

[ "$(uname)" = Darwin ] || { echo "build-xcframework.sh needs macOS (lipo, xcodebuild)" >&2; exit 1; }
command -v cargo >/dev/null || { echo "Rust is missing: https://rustup.rs" >&2; exit 1; }

# The objects must target the app's minimum macOS, or the linker warns on every file.
export MACOSX_DEPLOYMENT_TARGET=14.0
# Arti's directory downloads use xz. Build liblzma into the core rather than linking whatever
# the Mac happens to have (Homebrew's, or an older system copy): the app must not depend on it.
export LZMA_API_STATIC=1
cargo_profile=()
[ "$profile" = release ] && cargo_profile=(--release)

say() { printf '\033[1;35m▸\033[0m %s\n' "$*"; }

libs=()
for target in "${targets[@]}"; do
  say "Building asiri-ffi for $target ($profile)"
  rustup target add "$target" >/dev/null 2>&1 || true
  # (Empty arrays are expanded with ${a[@]+…}: macOS's bash 3.2 treats them as unset under -u.)
  cargo build -p asiri-ffi --locked --target "$target" ${cargo_profile[@]+"${cargo_profile[@]}"} ${features[@]+"${features[@]}"}
  libs+=("target/$target/$profile/libasiri_ffi.a")
done

work="$root/target/xcframework"
rm -rf "$work"
mkdir -p "$work/headers" "$work/swift"

say "Combining $(printf '%s ' "${targets[@]}")"
lipo -create "${libs[@]}" -output "$work/libasiri_ffi.a"

say "Generating the Swift bindings"
cargo build -q -p uniffi-bindgen --locked
"target/debug/uniffi-bindgen-swift" "target/${targets[0]}/$profile/libasiri_ffi.dylib" "$work/swift" \
  --swift-sources --headers --modulemap \
  --module-name AsiriCoreFFI --modulemap-filename module.modulemap
mv "$work/swift/AsiriCoreFFI.h" "$work/swift/module.modulemap" "$work/headers/"

say "Packaging AsiriCoreFFI.xcframework"
out="$root/apps/macos/Frameworks/AsiriCoreFFI.xcframework"
rm -rf "$out"
mkdir -p "$(dirname "$out")"
xcodebuild -create-xcframework -library "$work/libasiri_ffi.a" -headers "$work/headers" -output "$out" >/dev/null

generated="$root/apps/macos/Packages/AsiriCoreBridge/Sources/AsiriCoreKit/Generated"
mkdir -p "$generated"
cp "$work/swift/AsiriCoreKit.swift" "$generated/AsiriCoreKit.swift"

size=$(du -sh "$out" | cut -f1)
say "Done: apps/macos/Frameworks/AsiriCoreFFI.xcframework ($size) and the Swift bindings"
