# 121 — Upstream issue drafts (ScreenCaptureKit adoption)

**Drafts only. Nothing has been posted.** Each issue below is ready to paste
once the user approves it. Written 2026-10-07 by W3 DOCS of wave 121 from
`docs/design/121-sck-capture-spike.md` (§2, §4.3, §4.4, §6) and the fetched
crate sources under `~/.cargo/registry/src/*/`. Repositories are taken from
each crate's `Cargo.toml` `repository` field. Anything not checked against
source or a run is marked **(unverified)**. Our carried workaround is
`crates/computer/build.rs` (quoted verbatim where relevant).

Common environment (identical for all three):

```
macOS 26.6.2 (build 25G83), Apple Silicon
Command Line Tools only, /Library/Developer/CommandLineTools (no Xcode.app)
xcrun --sdk macosx --show-sdk-version → 26.5
swift 6.3.2 (swiftlang-6.3.2.1.108)
rustc / cargo 1.98.1
apple-metal 0.10.0 · apple-cf 0.11.0 · screencapturekit 11.0.0
```

---

## Issue 1 — apple-metal: link fails on Command Line Tools-only hosts (Swift lib search path hard-codes the Xcode toolchain)

**Repository:** https://github.com/doom-fish/apple-metal-rs (`apple-metal` 0.10.0 `Cargo.toml` `repository`)

**Environment:** see above.

**Steps to reproduce**

1. On a Mac with only the Command Line Tools installed (`xcode-select -p` →
   `/Library/Developer/CommandLineTools`), add
   `screencapturekit = { version = "11", default-features = false, features = ["macos_14_0"] }`
   (which depends on `apple-metal >=0.10, <0.11`) to a binary crate.
2. `cargo build` (or `cargo test` — anything that links a binary).

**Observed**

The three Swift bridges build, then the final link fails:

```
ld: Could not find or use auto-linked library 'swiftCompatibility56'
ld: Could not find or use auto-linked library 'swiftCompatibilityConcurrency'
ld: Could not find or use auto-linked library 'swiftCompatibilityPacks'
Undefined symbols:
  __swift_FORCE_LOAD_$_swiftCompatibility56 … referenced from AppleMetalSwiftBridge in libapple_metal-….rlib
```

**Cause (from the crate source)**

- `apple-metal` 0.10.0 `build.rs:71-79` adds `cargo:rustc-link-search=native=…`
  only for `<xcode-select -p>/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/macosx`.
  That directory exists under `/Applications/Xcode.app/Contents/Developer`
  and not under `/Library/Developer/CommandLineTools`, where the static
  `libswiftCompatibility*.a` live at `/Library/Developer/CommandLineTools/usr/lib/swift/macosx/`.
- The force-load references come from the bridge's deployment target:
  `swift-bridge/Package.swift` declares `.macOS(.v11)` and `build.rs:29-33`
  passes `--triple arm64-apple-macosx` / `x86_64-apple-macosx` with no OS
  version, so Swift emits back-deployment compatibility force-loads. The
  sibling bridges (`apple-cf`, `screencapturekit`) target 13.0 and need none.

**Expected**

`cargo build` links on a CLT-only host, as `screencapturekit`'s README
promises ("**Xcode Command Line Tools** at build time").

**Workaround we carry** (`crates/computer/build.rs`, our dependent crate)

```rust
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
```

**Proposed fix** (either, both is best)

1. Derive the directory from `xcrun --find swift` → `<toolchain>/usr/lib/swift/macosx`
   instead of `xcode-select -p` + the hard-coded `XcodeDefault.xctoolchain`
   path; this resolves correctly under both Xcode and the CLT.
2. Pin the bridge to `macosx13.0` like the sibling crates (`.macOS(.v13)` in
   `Package.swift`, `--triple arm64-apple-macosx13.0`), so no
   `swiftCompatibility*` force-loads are emitted at all **(that this removes
   every force-load for this bridge: unverified — inferred from the sibling
   bridges linking without them)**.

---

## Issue 2 — screencapturekit: dependents abort at launch (no `LC_RPATH`); and `capture_image` + `rgba_data` leaves a ~14 MB purgeable CoreGraphics cache per image

**Repository:** https://github.com/doom-fish/screencapturekit-rs (`screencapturekit` 11.0.0 `Cargo.toml` `repository`)

**Environment:** see above. Part A was observed on the CLT-only host after
working around Issue 1; Part B is host-independent as far as we can tell.

### Part A — `@rpath/libswift_Concurrency.dylib` not found in dependent binaries

**Steps to reproduce**

1. Depend on `screencapturekit` 11 from a binary crate `app` (`features = ["macos_14_0"]`).
2. `cargo build --release -p app`; `otool -l target/release/app | grep -A2 LC_RPATH` → nothing.
3. Run the binary.

**Observed**

```
dyld[1909]: Library not loaded: @rpath/libswift_Concurrency.dylib
  Reason: no LC_RPATH's found
```

The same happens for every `cargo test` binary of every crate that depends on
`app`'s library crate. `install_name_tool -add_rpath /usr/lib/swift` on a copy
makes it run.

**Cause**

`build.rs:305` emits `cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift` (and
`:313` the Xcode toolchain rpath). Cargo applies `rustc-link-arg` only to the
emitting package's own binaries (tests, examples, bins of `screencapturekit`
itself), never to dependents — so the rpath is lost for every real user, while
the bridge's `-lswift_Concurrency` still resolves to the toolchain's
`@rpath/libswift_Concurrency.dylib`. README `## Requirements & Permissions`
has no mention of an rpath.

**Expected**

A dependent binary runs after `cargo build`, or the README/Install section says
what every dependent must add.

**Workaround we carry** — in the `build.rs` of every crate that links the
library using `screencapturekit`:

```rust
println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
```

**Proposed fix**

1. Document the required `-Wl,-rpath,/usr/lib/swift` (per dependent binary
   `build.rs`, or `[target.aarch64-apple-darwin] rustflags` in
   `.cargo/config.toml`) under Install / Requirements, next to the CLT note.
2. And/or link the Swift runtime against the SDK's `.tbd` stubs so that
   `libswift_Concurrency` resolves to `/usr/lib/swift/libswift_Concurrency.dylib`
   (in the dyld shared cache since macOS 12) instead of an `@rpath` reference
   **(feasibility with the back-deployment copy that the 11.0 `apple-metal`
   bridge pulls in: unverified)**. Fixing Issue 1 (bridge at 13.0) may remove
   the `@rpath` reference on its own **(unverified)**.

### Part B — `SCScreenshotManager::capture_image` + `CGImageExt::rgba_data` grows max RSS by ~14 MB per shot

**Steps to reproduce** (2560×1440 display, release build)

Loop N times: `SCScreenshotManager::capture_image(&filter, &config)?.rgba_data()?`,
drop both; measure with `/usr/bin/time -l`, then `vmmap -summary` while held.

**Observed**

| rounds | 1 | 5 | 20 | 60 |
|---|---|---|---|---|
| max RSS, `capture_image` + `rgba_data` | 63.7 MB | 123.2 MB | 345.1 MB | 522.4 MB |
| max RSS, `capture_sample_buffer` → `CVPixelBuffer` row copy | 48.5 MB | 48.8 MB | 49.0 MB | 49.0 MB |

`vmmap -summary` after 60 rounds: `MALLOC_LARGE` 451.0 MB virtual in
`DefaultPurgeableMallocZone`, 32 allocations of ~14.1 MB, 0 K resident / 0 K
dirty once idle; physical footprint stays ~20 MB. Isolation runs: dropping the
`CGImage` per round without rendering is flat (19 MB); rendering the *same*
`CGImage` repeatedly is flat (63.8 MB); wrapping in an autorelease pool
changes nothing. So it is CoreGraphics' per-image decode cache from rendering
each new `CGImage` through the `CGContext` in `apple-cf`'s
`cgimage_render_rgba_into` — purgeable, not a leak, but it shows as hundreds of
MB of RSS in Activity Monitor for a long-lived process. The render also
converts to device RGB: mean |ΔRGB| 1.48 vs the raw frame (menu-bar rows ≈ 13.5).
**(Attribution to the CG decode cache is from the isolation runs and `vmmap`,
not from Apple documentation — unverified beyond that.)**

**Expected**

Either flat RSS from the documented screenshot route, or a doc note steering
screenshot users to `capture_sample_buffer` + `CVPixelBuffer` for raw pixels.

**Proposed fix**

1. README/doc note on `CGImageExt::rgba_data`: per-image purgeable CG cache;
   prefer `capture_sample_buffer` → `CMSampleBufferExt::pixel_buffer()` →
   `lock_read_only()` + stride-aware copy for pixel access (that is what we
   ship; it is also faster here: 49.4 vs 68.6 ms median).
2. And/or an `rgba_data` implementation that reads the `CGImage`'s data
   provider / backing `IOSurface` directly instead of drawing through a
   `CGContext`, avoiding the decode cache **(unverified that the SCK-produced
   `CGImage` always exposes a directly readable BGRA provider)**.

---

## Issue 3 — screencapturekit: make `apple-metal` optional behind a feature

**Repository:** https://github.com/doom-fish/screencapturekit-rs

**Environment:** see above.

**Steps to reproduce**

`cargo build -p <crate depending on screencapturekit 11, default-features = false, features = ["macos_14_0"]>`
on a clean target dir and time the build scripts; `ls -l target/*/build/*/out/**/release/*.a`.

**Observed**

- `[dependencies.apple-metal]` is unconditional (`Cargo.toml:371-373`,
  `version = ">=0.10, <0.11"`, `default-features = false`), so every user pays
  its Swift bridge: **26.5 s** `swift build -c release` (apple-cf 27.3 s,
  screencapturekit 35.7 s; cargo overlaps them to 63–76 s cold, also under
  `cargo check`), and a **2 078 792 B** `libAppleMetalSwiftBridge.a`
  (vs 900 896 B `libAppleCFBridge.a`, 1 788 176 B `libScreenCaptureKitBridge.a`).
- In `screencapturekit`'s own source `apple_metal` is referenced only from
  `src/metal.rs` and the `pub use apple_metal;` re-export in `src/lib.rs:756`
  (`grep -rl apple_metal src/`). Screenshot (`SCScreenshotManager`) and
  `CVPixelBuffer`/`IOSurface` stream users never touch it.
- The `apple-metal` bridge is also the one pinned to macOS 11 and therefore
  the source of the `swiftCompatibility*` force-loads and the CLT link failure
  (Issue 1).

**Expected**

`screencapturekit` without Metal interop builds two bridges, not three.

**Workaround we carry**

None possible from the dependent side; we carry Issue 1's link-search shim and
accept the build time.

**Proposed fix**

```toml
[features]
metal = ["dep:apple-metal"]

[dependencies.apple-metal]
version = ">=0.10, <0.11"
default-features = false
optional = true
```

with `#[cfg(feature = "metal")] pub mod metal;` and
`#[cfg(feature = "metal")] pub use apple_metal;` in `src/lib.rs`, and the
Metal examples (`16_full_metal_app`, `17_metal_textures`, `18_wgpu_integration`)
and `metal_tests` gated by `required-features = ["metal"]`. Whether `metal`
belongs in `default` is the maintainer's call; our ask is only that it can be
turned off. **(That nothing else in `src/` reaches `apple_metal` through
`apple-cf` re-exports: unverified beyond the grep above.)**
