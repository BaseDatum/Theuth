//! ScreenCaptureKit link fixes (121 §6): the Swift bridges of
//! `screencapturekit` / `apple-cf` / `apple-metal` force-load the static
//! `libswiftCompatibility*.a` libraries, and `apple-metal` 0.10's build.rs only
//! adds the link-search path for them under a full Xcode
//! (`<xcode-select -p>/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/macosx`).
//! On a Command Line Tools-only host that directory does not exist and the
//! final link fails with `__swift_FORCE_LOAD_$_swiftCompatibility56` undefined.
//! Resolve the directory from the active `swift` instead, which works for both.
//!
//! Second gap: the bridges' `cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift`
//! only applies to *their own* binaries (Cargo never propagates link-arg to
//! dependents), yet `-lswift_Concurrency` resolves to the toolchain's
//! `@rpath/libswift_Concurrency.dylib`. Without the rpath every binary that
//! links this crate aborts at launch with `Library not loaded:
//! @rpath/libswift_Concurrency.dylib`. The link-arg below covers this crate's
//! tests and examples; a shipping binary (`theuth`) needs the same flag itself.

fn main() {
    println!("cargo:rerun-if-env-changed=DEVELOPER_DIR");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    let Ok(out) = std::process::Command::new("xcrun")
        .args(["--find", "swift"])
        .output()
    else {
        return;
    };
    if !out.status.success() {
        return;
    }
    let swift = std::path::PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    // <toolchain>/usr/bin/swift → <toolchain>/usr/lib/swift/macosx
    if let Some(usr) = swift.parent().and_then(|bin| bin.parent()) {
        let lib = usr.join("lib/swift/macosx");
        if lib.is_dir() {
            println!("cargo:rustc-link-search=native={}", lib.display());
        }
    }
}
