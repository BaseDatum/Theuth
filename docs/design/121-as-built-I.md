# 121 — As built, integration (wave 121 SCK-ADOPTION, 2026-10-07)

Status: integrated and landed 2026-10-07. Branch `feat/121-sck-capture`, base d06b08c3 = main 5bc474b9 + the six
spike-121 commits (cherry-picked clean; `cargo metadata --locked` consistent). Workers: W1 CAPTURE
(`feat/121-w1-capture`, tip 27453ae9), W2 LINK (`feat/121-w2-link`, 28eed227), W3 DOCS (`feat/121-w3-docs`,
a613f37c), all omp ACP children spawned 08:46 from briefs under `~/.theuth/briefs/2026-10-07/121/`; all three
reported DONE by 10:00. Per-worker records: `121-as-built-W1-CAPTURE.md`, `121-as-built-W2-LINK.md`,
`121-as-built-W3-DOCS.md`; the spike report is `121-sck-capture-spike.md`; design 27 carries the dated Revision.

## 1. What landed

- `crates/computer`: ScreenCaptureKit (`screencapturekit` 11, `macos_15_0`) is the default macOS screenshot path
  (`SCScreenshotManager::capture_sample_buffer` → CVPixelBuffer → RGBA); `/usr/sbin/screencapture` is the automatic
  per-call fallback on any SCK error except a lost Screen Recording grant; `THEUTH_CAPTURE_BACKEND=screencapture|sck`
  forces one path for diagnosis; public API unchanged; `build.rs` adds the Command-Line-Tools Swift link-search and
  the crate's own test rpath. Tests 43 → 46.
- `crates/theuth`, `crates/rpc`, `crates/tui`: new `build.rs` emitting `-Wl,-rpath,/usr/lib/swift` on macOS — every
  binary that links `computer` needs it (Cargo does not propagate a dependency's `rustc-link-arg`).
- `crates/desktop/Info.plist`: `NSScreenCaptureUsageDescription`.
- `scripts/package-macos.sh`: `check_sidecar` (otool `LC_RPATH /usr/lib/swift` + `t --version`) on the plain release
  build and again on the codesigned sidecar (integrator's commit 9698edd1, after Sky asked that signing/packaging be
  handled in the scripts).
- Docs: design 27 Revision 2026-10-07 + three pointers; spike §10 Outcome; `121-upstream-issues.md` (three drafts,
  not posted); the three worker as-builts.

## 2. Integration sequence

1. Merge order W3 (697c9498) → W1 (834f2726) → W2 (ecd125a0): disjoint file sets, zero conflicts.
2. Fold of main e04f1d1a (8 commits: v2.9.3, 120 Revision 1, 126/127 docs) → 237049ef; `cargo metadata --locked` OK.
3. `package-macos.sh` guard → 9698edd1.

## 3. Gates on the integration tree (own target dir `~/Develop/theuth-121-integ-target`, APFS clone; every step
through the machine-wide `build-lock`, `CARGO_BUILD_JOBS=4`, `CARGO_INCREMENTAL=0`; detached with `setsid` so Sky's
10:25 engine restart did not interrupt it)

| Step | Result |
|---|---|
| `cargo build -p theuth` (debug, BUILD_SLOTS=1) | exit 0 (waited 807 s for a slot) |
| `target/debug/t --version` | `t 2.9.3`, exit 0 |
| `otool -l t \| grep -A2 LC_RPATH` | `path /usr/lib/swift (offset 12)` |
| `cargo test -p computer` | 46 passed, 0 failed |
| `cargo test -p rpc --lib -- --list` | exit 0, 41 tests listed (test binary launches) |
| `cargo clippy -p computer --all-targets -- -D warnings` | clean |
| Signed-sidecar proof: copy of `t`, `codesign --force --options runtime --sign - -i io.theuth.mac.t --entitlements crates/desktop/entitlements.plist` | flags `adhoc,runtime`; `t 2.9.3` exit 0; `check_sidecar` PASS |
| Negative control: `install_name_tool -delete_rpath /usr/lib/swift` on a copy | `check_sidecar` fails with `links the Swift runtime but carries no LC_RPATH /usr/lib/swift` |
| `bash -n` + `shellcheck -S warning scripts/package-macos.sh` | clean |

W1's own gates (fmt, clippy, 46 tests, bench parity: dims equal, mean |ΔRGB| 0.001, default path through SCK, no
fallback line) and W2's (12m37s `t` build, negative control exit 134 with the dyld error, rpc 41 / tui 129 tests listed)
are in their as-builts.

## 4. Deviations from the briefs

- W1: no `--release` bench (clone had no release cache); debug numbers only (SCK 50.7 ms median vs 231.4 for the
  shell-out on this run); one extra test for the lost-grant branch.
- W2: reverted a pre-existing rustfmt drift in `crates/theuth/src/mcp.rs` that `cargo fmt -p theuth` surfaced (not
  committed); Info.plist key added because Apple's framework page lists it.
- W3: caching follow-up written as 11–24 ms (spike §4.2/§7.4) instead of the brief's 14–24.
- Integrator: the `package-macos.sh` guard was not in any brief; added after Sky's 10:26 instruction.

## 5. Open items

- Retina / scale-2 / multi-display behaviour unverified: only a 2560x1440 scale-1 display was attached all day.
  First check with the lid open: `cargo run -p computer --example capture_bench -- --save /tmp/x` and compare
  dimensions and crops.
- `package-macos.sh` not run end to end (needs the signing identity and the Tauri build); `check_sidecar` is proven
  on an ad-hoc hardened-runtime signature of the debug `t`, not on a Developer ID release bundle.
- Engine rebuild owed: the running engine predates this change (and migration 0033).
- Upstream issues (`121-upstream-issues.md`) await Sky's approval before posting.
- Follow-up worth 11–24 ms per grab: cache the `SCDisplay`/filter per process with refresh-on-miss.
- `x86_64-apple-darwin` build unexercised; release-profile `t` rebuilt only in the spike (then with
  `install_name_tool`), not with the new `build.rs`.
