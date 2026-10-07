# 121 as-built — W2 LINK, Swift runtime rpath for every binary that links `computer`

Branch `feat/121-w2-link`, cut from `d06b08c3`. Commits `8540f13e` (three build.rs), `7aa66e7f` (Info.plist), plus this note. Base context: docs/design/121-sck-capture-spike.md §6 items 3, 4, 6.

## What changed

- `crates/theuth/build.rs` (new, 19 lines), `crates/rpc/build.rs` (new, 19 lines), `crates/tui/build.rs` (new, 19 lines) — byte-identical. L15 `cargo:rerun-if-changed=build.rs`; L16-18: when `CARGO_CFG_TARGET_OS == "macos"`, `cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift`. L1-12 module comment: why (Swift bridges reference `@rpath/libswift_Concurrency.dylib`, Cargo does not propagate `rustc-link-arg`), the failure mode, the rule, pointers to spike §6 and this note.
- `crates/desktop/Info.plist` L11-13 — `NSScreenCaptureUsageDescription` added after `NSAudioCaptureUsageDescription` (D3, below). `plutil -lint`: OK.
- No Cargo.toml edits: Cargo picks up a package-root `build.rs` automatically. Cargo.lock untouched (`git status` clean after every cargo command).

## The rule for future crates

ANY crate that links `computer` directly or transitively and produces a binary (bin, test, example, bench) needs the same `build.rs`. Cargo applies `cargo:rustc-link-arg` only to the emitting package's own artifacts; `cargo:rustc-link-search` is propagated, `rustc-link-arg` is not. `crates/computer/build.rs` therefore fixes the CLT link-search for everyone but the rpath only for `computer`'s own tests/examples. Check with `cargo tree -p <bin-crate> -i computer --target aarch64-apple-darwin`; the symptom of forgetting is an immediate `dyld … Library not loaded: @rpath/libswift_Concurrency.dylib … Reason: no LC_RPATH's found` abort (exit 134) on launch.

## D2 evidence (verbatim)

All commands via `BUILD_LABEL=121-W2 build-lock`, `CARGO_TARGET_DIR=/Users/Sky.Moore/Develop/theuth-121-w2-target`, `CARGO_INCREMENTAL=0`, `CARGO_BUILD_JOBS=4`, cargo 1.98.1.

### (a) `cargo build -p theuth` (debug, BUILD_SLOTS=1)

`Finished `dev` profile [unoptimized + debuginfo] target(s) in 12m 37s`, exit 0.

```
$ $CARGO_TARGET_DIR/debug/t --version
t 2.9.2
exit 0

$ otool -l $CARGO_TARGET_DIR/debug/t | grep -A2 LC_RPATH
          cmd LC_RPATH
      cmdsize 32
         path /usr/lib/swift (offset 12)

$ otool -L $CARGO_TARGET_DIR/debug/t | grep -i swift
	/usr/lib/swift/libswiftAVFoundation.dylib (compatibility version 1.0.0, current version 2420.7.1, weak)
	/usr/lib/swift/libswiftCore.dylib (compatibility version 0.0.0, current version 0.0.0)
	/usr/lib/swift/libswiftCoreAudio.dylib (compatibility version 1.0.0, current version 411.255.0, weak)
	/usr/lib/swift/libswiftCoreFoundation.dylib (compatibility version 1.0.0, current version 120.100.0)
	/usr/lib/swift/libswiftCoreImage.dylib (compatibility version 1.0.0, current version 2.2.0, weak)
	/usr/lib/swift/libswiftCoreMIDI.dylib (compatibility version 1.0.0, current version 6.0.0, weak)
	/usr/lib/swift/libswiftCoreMedia.dylib (compatibility version 1.0.0, current version 3320.8.5, weak)
	/usr/lib/swift/libswiftDarwin.dylib (compatibility version 1.0.0, current version 377.120.8, weak)
	/usr/lib/swift/libswiftDispatch.dylib (compatibility version 1.0.0, current version 1542.100.32)
	/usr/lib/swift/libswiftIOKit.dylib (compatibility version 1.0.0, current version 1.0.0, weak)
	/usr/lib/swift/libswiftMetal.dylib (compatibility version 1.0.0, current version 373.2.0)
	/usr/lib/swift/libswiftOSLog.dylib (compatibility version 1.0.0, current version 10.0.0, weak)
	/usr/lib/swift/libswiftObjectiveC.dylib (compatibility version 1.0.0, current version 951.7.0)
	/usr/lib/swift/libswiftQuartzCore.dylib (compatibility version 1.0.0, current version 5.0.0, weak)
	/usr/lib/swift/libswiftSpatial.dylib (compatibility version 1.0.0, current version 1.0.0, weak)
	/usr/lib/swift/libswiftUniformTypeIdentifiers.dylib (compatibility version 1.0.0, current version 877.5.1)
	/usr/lib/swift/libswiftVideoToolbox.dylib (compatibility version 1.0.0, current version 3320.8.5, weak)
	/usr/lib/swift/libswiftXPC.dylib (compatibility version 1.0.0, current version 128.120.2, weak)
	/usr/lib/swift/libswiftos.dylib (compatibility version 1.0.0, current version 1082.0.0, weak)
	/usr/lib/swift/libswiftsimd.dylib (compatibility version 1.0.0, current version 23.0.0, weak)
	@rpath/libswift_Concurrency.dylib (compatibility version 0.0.0, current version 0.0.0)
	/usr/lib/swift/libswiftFoundation.dylib (compatibility version 1.0.0, current version 1.0.0)
	/usr/lib/swift/libswiftCoreGraphics.dylib (compatibility version 1.0.0, current version 1.0.0)
```

23 Swift load commands; 22 absolute `/usr/lib/swift/...`, one `@rpath/libswift_Concurrency.dylib` — the only line the LC_RPATH exists for.

### (b) Negative control

`crates/theuth/build.rs` L17 commented out; `cargo build -p theuth` relinked only `theuth` (`Finished … in 55.98s`, exit 0). `otool -l … | grep -A2 LC_RPATH` printed nothing (no LC_RPATH at all).

```
$ $CARGO_TARGET_DIR/debug/t --version
dyld[16753]: Library not loaded: @rpath/libswift_Concurrency.dylib
  Referenced from: <F1AA63A2-7C1C-3C83-87C9-38AE198EA056> /Users/Sky.Moore/Develop/theuth-121-w2-target/debug/t
  Reason: no LC_RPATH's found
exit 134
```

Line restored via `git checkout -- crates/theuth/build.rs`; rebuilt (`Finished … in 2m 40s`, exit 0); `otool -l` again shows `path /usr/lib/swift (offset 12)`; `t --version` → `t 2.9.2`, exit 0.

### (c) Test binaries of `rpc` and `tui`

```
$ cargo test -p rpc --lib -- --list        # exit 0
Running unittests src/lib.rs (…/debug/deps/rpc-6ef9e7b6adb4b592)
acp::tests::acp_cost_only_for_complete_single_currency: test
acp::tests::child_session_over_the_mesh_loopback: test
acp::tests::describe_names_a_vm_engine: test
acp::tests::image_block_becomes_sha_named_file: test
acp::tests::plan_keeps_blocked_and_dropped_honest: test
acp::tests::session_new_meta_becomes_create_options: test
api::tests::app_calls_need_an_authenticated_peer: test
api::tests::communication_changed_reaches_scoped_and_tree_subscribers: test
…
41 tests, 0 benchmarks

$ cargo test -p tui --lib -- --list        # exit 0
Running unittests src/lib.rs (…/debug/deps/tui-fa1229b9b3fc2e5a)
attach::tests::session_fallbacks_follow_the_naming_table: test
attach::tests::sizes_and_display: test
attach::tests::spoken_marker: test
clip::tests::downscale_averages_blocks_and_missing_files_are_not_images: test
dictation::tests::meter_pads_left_and_scales: test
dictation::tests::set_utt_grows_and_levels_cap: test
…
129 tests, 0 benchmarks
```

`otool -l` on both test binaries: `path /usr/lib/swift (offset 12)`. (A first tui listing piped through `head -8` returned 101 — cargo's stdout broken pipe, not the test binary; rerun to a file is the exit 0 above.) rpc's full suite was not run, per brief. `cargo test -p computer` is W1's gate, not run.

## D3 — Info.plist

Apple's ScreenCaptureKit framework page (https://developer.apple.com/documentation/screencapturekit, Overview) states: "Request screen recording permission from the person before capturing content. In the Info pane of the Xcode target editor, add a `NSScreenCaptureUsageDescription` key with a description of why your app requires screen recording access." The article "Capturing screen content in macOS" (https://developer.apple.com/documentation/ScreenCaptureKit/capturing-screen-content-in-macos) only says the system prompts for Screen Recording on first run and does not name the key. Listed → added to `crates/desktop/Info.plist` L11-13 with the brief's wording.

Attribution note: the daemon `t` runs as a Tauri sidecar inside theuth.app (TCC attributes the Screen Recording grant to the app bundle, so the bundle's plist is the one that matters) or from a terminal (attributed to the terminal, which has no plist of ours). The preflight in `computer` is unchanged. Whether the key's absence would block a CLI sidecar at all remains unverified (spike §6 item 6); adding it is the documented path and costs nothing.

## D4 — packaging path

Nothing strips or rewrites load commands; no change made.

- `scripts/package-macos.sh:51` `cargo build --release -p theuth`; `:61` `cp target/release/t "crates/desktop/binaries/t-$HOST_TRIPLE"` — the sidecar is the cargo-built `t`, copied verbatim. `:67` `cargo tauri build --config tauri.bundle.conf.json`; `:69` asserts `Contents/MacOS/t` exists. `:103-105` `codesign --force --options runtime … -i io.theuth.mac.t --entitlements crates/desktop/entitlements.plist "$APP/Contents/MacOS/t"` — codesign preserves LC_RPATH. `:106-108` app-level sign + `--verify --deep --strict`. `:115-118` `ditto` + `hdiutil create` (byte copies). No `install_name_tool`, `strip`, `lipo` anywhere in `scripts/package-macos.sh`, `scripts/notarize-macos.sh`, `scripts/package-update-macos.sh`, `scripts/install-macos.sh` (grep). The "stripped" in `package-macos.sh:2` describes the pipeline, not binaries; root `Cargo.toml:449-460` profiles set no `strip`.
- `scripts/notarize-macos.sh:38-42` `notarytool submit`, `stapler staple`, `spctl --assess` — no binary mutation.
- `crates/desktop/tauri.bundle.conf.json:5` `"externalBin": ["binaries/t"]`, `:7-8` `hardenedRuntime: true`, `entitlements: ./entitlements.plist`. `crates/desktop/tauri.macos.conf.json:41` `minimumSystemVersion: "15.0"` (≥ macOS 12.0 where `libswift_Concurrency` entered the shared cache). `crates/desktop/entitlements.plist` has `allow-jit` and `audio-input` only; the hardened runtime does not restrict loading system Swift dylibs from `/usr/lib/swift`.
- `crates/desktop/Cargo.toml` has no `computer` dependency (grep `computer` → no match), so `theuth-desktop` needs no build.rs.

## D5 — who links `computer`

`cargo tree -p theuth -i computer --target aarch64-apple-darwin` (exit 0):

```
computer v2.9.2 (/Users/Sky.Moore/Develop/theuth-121-w2/crates/computer)
├── rpc v2.9.2 (/Users/Sky.Moore/Develop/theuth-121-w2/crates/rpc)
│   ├── theuth v2.9.2 (/Users/Sky.Moore/Develop/theuth-121-w2/crates/theuth)
│   └── tui v2.9.2 (/Users/Sky.Moore/Develop/theuth-121-w2/crates/tui)
│       └── theuth v2.9.2 (/Users/Sky.Moore/Develop/theuth-121-w2/crates/theuth)
├── theuth v2.9.2 (/Users/Sky.Moore/Develop/theuth-121-w2/crates/theuth)
└── tui v2.9.2 (/Users/Sky.Moore/Develop/theuth-121-w2/crates/tui) (*)
```

Exactly `rpc`, `tui`, `theuth` — all three now carry the build.rs. `grep '^computer' crates/*/Cargo.toml` agrees (rpc:23, theuth:23, tui:19). No other dependent.

## Rejected alternative

`.cargo/config.toml` `[target.*-apple-darwin] rustflags = ["-C", "link-arg=-Wl,-rpath,/usr/lib/swift"]`: a global flag that changes the rustflags hash and so invalidates every target dir on the machine (several sessions share this MacBook), and is silently dropped whenever a `RUSTFLAGS` env is set (env overrides config, no merge). Three 19-line build.rs files are local, explicit, and visible next to the crate that needs them.

## Deviations

- `cargo fmt -p theuth -p rpc -p tui` also reformatted `crates/theuth/src/mcp.rs:261-266` (pre-existing drift, not mine); reverted with `git checkout` so the commit touches only owned files. The three build.rs were already rustfmt-clean.
- None otherwise; no Cargo.toml / Cargo.lock / crates/computer edits; packaging scripts unchanged.

## Open items

- `x86_64-apple-darwin` build unexercised (host arch only; `package-macos.sh:15` is arm64-only).
- Release-profile `t` not rebuilt here (brief: debug only); the link-arg is profile-independent, and the spike's release observation (§6 item 3) is the same dyld error this debug negative control reproduces.
- Whether `NSScreenCaptureUsageDescription` is consulted by TCC for a non-GUI sidecar remains unverified; the key is present in the bundle plist regardless.
- Packaging scripts not run (per brief); the `codesign`-preserves-LC_RPATH claim is from the tool's documented behaviour, not re-measured on a signed bundle here.
