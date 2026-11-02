//! `uniffi-bindgen-swift`: generates the Swift bindings, C header and module map for
//! `asiri-ffi` (used by tools/release/build-xcframework.sh).

fn main() {
    uniffi::uniffi_bindgen_swift();
}
