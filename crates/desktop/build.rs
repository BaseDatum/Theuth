//! Tauri build script: embeds `tauri.conf.json` and the capability set.
fn main() {
    // tauri-build embeds icons/icon.ico as the exe's Windows icon resource but does not tell Cargo to
    // re-run when it changes, so a cached build (CI rust-cache) can keep shipping the old icon.
    println!("cargo:rerun-if-changed=icons/icon.ico");
    tauri_build::build();
}
