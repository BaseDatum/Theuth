//! Swift runtime rpath for every binary of this package (wave 121, W2 LINK).
//!
//! `computer` links ScreenCaptureKit through Swift bridges whose objects
//! reference `@rpath/libswift_Concurrency.dylib`. Cargo never propagates a
//! dependency's `cargo:rustc-link-arg` to dependents (only `rustc-link-search`
//! is propagated), so without the line below every binary, test, example and
//! bench of this package aborts at launch with
//! `dyld: Library not loaded: @rpath/libswift_Concurrency.dylib ...
//! Reason: no LC_RPATH's found`.
//! Rule: ANY crate that links `computer` directly or transitively needs this
//! build.rs. See docs/design/121-sck-capture-spike.md §6 and
//! docs/design/121-as-built-W2-LINK.md.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
}
