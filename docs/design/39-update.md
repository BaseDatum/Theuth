# 39 — Update: check, verify, replace; never ask

Status: draft v0 · 2026-09-17 · extends `00-architecture.md` §2 (process
model), §7 (RPC), §9 (config), §10 (workspace); `25-desktop.md` §5
(packaging: "Updater. Not in v0"); `18-app-launch.md` §3 (lifecycle).
Where this doc disagrees with those, this doc wins; §13 lists the edits.

The user never sees a dialog, a toast, a modal, a snooze button or a
"Restart now?" prompt. A newer build lands on disk by itself, verified, and
takes effect the next time the thing restarts anyway. The only visible
trace is one dim tag in the TUI header and one dot on the desktop's
Settings entry.

## 0. What jarvis built, and what this doc keeps

Mapped 2026-09-17 against `../jarvis` (file:line in the table).

| | jarvis | here |
|---|---|---|
| Who updates | the Tauri shell only (`ui/src-tauri/src/updater.rs`, 966 lines); `jarvisd` and the TUI cannot update themselves, `jarvisd` rides inside the `.app`; macOS arm64 only | **every install on macOS arm64 and Linux x86_64/aarch64**: the `.app` (desktop), a `theuth` from `just install` / a tarball, a Homebrew `theuth` (formula on both OSes, cask on macOS — §3.5), and a `--install-service` daemon on a headless box (§3.4) |
| Transport | `tauri-plugin-updater` 2.10 reading a static `latest.json` from a public rustfs bucket, minisign-verified (`tauri.conf.json:48-54`) | same plugin for the `.app`; the same manifest, key and format for the CLI, verified with the same `minisign-verify` crate the plugin uses (§2.1) |
| UX | `UpdateToast.tsx`, `UpdateModal.tsx` (notes, progress, install/cancel/snooze/dismiss), `UpdatesPanel.tsx`, tray label `Update to vX…`, gear badge, post-relaunch success toast; `localStorage jarvis.updater {dismissed, snoozedUntil, toastShown}` | one header tag, one transcript line, one dot, one Settings row (§4). Nothing to dismiss because nothing interrupts |
| Apply | user clicks → download to memory → `ditto` backup → stop `jarvisd` (launchctl / SIGTERM / `lsof :7777` + `ps` comm match) → `install` → phase file → `/usr/bin/open` relaunch → `recover_on_boot` (`updater.rs:377-521, 843-931`); the stale pre-update daemon is found by **inode** (`desktop.rs:131-135, 219-223, 283-284, 439-441, 1094-1096` — five comments call it a recurring bug) | CLI: atomic rename over the binary the moment it verifies; the running process keeps its mapped image. Desktop: `install()` inside the quit path that already stops the sidecar (§3.3). No daemon hunting, no phase file, no relaunch, no inode |
| Phase machine | 9 phases persisted to `updater-state.json`, tray Restart/Quit locked during critical phases (`updater.rs:98-171, 523-548`) | none. Every step is either "nothing changed" or "the new bytes are in place" |
| Manifest arch names | DMG `arm64` (`uname -m`), tarball `aarch64` (Tauri) — `release-s3.sh:122-128` | one spelling, the Rust `std::env::consts` pair, mapped once in the script (§5) |
| Endpoint | hardcoded in Rust and in the script, "keep in sync by hand" (`updater.rs:24-25`) | one const in `crates/update`, one env default in the script, the same string; `update.url` overrides for a scratch bucket (§7) |
| Release gate | K16 (refuse to publish `latest.json` unless notarized) designed, **not implemented**; ad-hoc builds can publish production manifests | `release-s3.sh` refuses `latest.json` unless `spctl --assess` passes on the `.app` and `codesign -dv` shows a Developer ID (§5.2) |
| Post-upload check | none | script downloads what it uploaded and verifies each signature against the committed pubkey before writing `latest.json` (§5.2) |
| Config | none in `jarvis.toml`; policy in `localStorage` | `update.enabled`, `update.url` in `config.yaml` (§7); constants in code |
| Cadence | 12 h + 0–120 s jitter, from the SPA (`store.ts:140-159`) | 60 s after start, then every 6 h + 0–5 min jitter, from the process that owns the binary (§9) |

Kept from jarvis: the bucket (`s3.rwx.dev`, path-style, public `GetObject`
on `releases/*`, authenticated `PUT`), the `latest.json` shape, minisign via
`tauri signer sign`, the `COPYFILE_DISABLE=1 tar --no-xattrs` rule
(`package-updater-macos.sh:152-158`; AppleDouble entries break the plugin's
unpack), the rustfs `aws` CLI env (`release-s3.sh:164-174`), the ops probe
`test-s3-rustfs.sh`.

## 1. Decisions in one screen

1. **Silent by construction.** No code path in this doc can open a window,
   raise a notification, or block a quit for more than the install itself.
   Jarvis's toast/modal/snooze exist to manage an interruption; remove the
   interruption and the machinery goes with it.
2. **The process that owns a binary updates that binary.** `theuth` updates
   `current_exe()`; `theuth-desktop` updates `theuth.app` (which contains its
   sidecar `theuth`). Two small updaters in two processes, no cross-process
   orchestration. Rejected: the harness pushing `UpdateReady` to the desktop
   — the app must compare against *its own* version anyway, and a reused
   terminal daemon (18 §3) may be a different version than the app.
3. **One manifest, one key, one signature tool** for both artifacts. The
   Tauri static format has `platforms: HashMap<String, {url, signature}>`
   (plugin `updater.rs:78-88`); extra keys are ignored, so the CLI tarballs
   live under `cli-<os>-<arch>` in the same `latest.json`.
4. **Apply at the boundary that exists.** CLI: on disk immediately, live at
   the next `theuth` start. Desktop: at quit, after the sidecar is stopped.
   Service daemon: `exit(75)` when nothing is attached, the service manager
   restarts it (18 §3 "no auto-restart loop" stands for the desktop-spawned
   daemon: it is never restarted from inside). Rejected: relaunch-in-place
   (jarvis's `open` + `recover_on_boot`): 200 lines that exist only to avoid
   waiting for a restart the user will do anyway.
5. **Nothing is "available"; it is "ready".** The check, download and
   verification are one step. Only after the bytes are verified and (CLI) in
   place does any UI change. A failed check is a `debug!` line, never a UI
   state. Rejected: jarvis's `available → downloading → …` phases the UI
   has to render.
6. **Failure leaves the old version untouched and says nothing.** Rename is
   atomic; Tauri's install renames the old bundle aside before moving the
   new one in (§2.2). The next tick retries. The user is not the operator
   of the update system.
7. **Never downgrade; never cross a major.** `remote > current` under semver
   with the extra rule `remote.major == current.major` (a config or sqlite
   schema break is a major; a major ships as a DMG and a `just install`,
   not as a silent swap). Rollback of a bad minor is a higher minor.
8. **Signed the same way the sidecar is.** The CLI tarball carries the
   exact `Contents/MacOS/t` from the notarized `.app` (Developer ID,
   hardened runtime, `io.theuth.mac.t`), so a downloaded `theuth` runs under
   Gatekeeper without a quarantine dance. One macOS build, two containers;
   Linux binaries carry only the minisign signature, which is all Linux asks.
9. **`crates/update` is a plugin** (00 §4): one actor, one event, one `Api`
   ask. The desktop does not link it (25 §0.1); it shares only
   `crates/update/pubkey.txt` via `include_str!`.
10. **A package manager's file is the package manager's.** A `theuth` under
    `/Cellar/theuth/` is never renamed over; the harness runs `brew upgrade
    theuth` itself, quietly, after our release bumped the tap (§3.5). The
    `.app` from the cask self-updates and the cask says so
    (`auto_updates true`), which is Homebrew's own convention for apps that
    update themselves (Chrome, VS Code, …); `brew upgrade` then leaves it
    alone. Rejected: renaming inside the Cellar — works, and leaves `brew
    list --versions` lying about what is installed until the next `brew
    upgrade` reinstalls what is already there.
11. **Linux is a first-class CLI target, not a desktop one.** `linux-x86_64`
    and `linux-aarch64` tarballs of `theuth`, built on the oldest glibc we
    support (§5.1), published in the same manifest, installed by the same
    §3.2 code path. 25 is macOS-only and stays so; a Linux desktop is not
    in this doc.

## 2. Facts relied on

### 2.1 `tauri-plugin-updater` (plugins-workspace `v2`, `plugins/updater/src/updater.rs`, read 2026-09-17)

- Static manifest parse: `RemoteReleaseInner::Static { platforms:
  HashMap<String, ReleaseManifestPlatform> }` (L78-88); unknown keys are
  just more entries. Lookup order for macOS `.app`: `darwin-aarch64-app`,
  then `darwin-aarch64` (`get_urls`, L616-640).
- `check()` compares `release.version > current_version` unless
  `version_comparator` is set (L583-586). `current_version` is
  `app.package_info().version` = workspace `0.1.0`.
- `download(on_chunk, on_finish) -> Vec<u8>` verifies the minisign
  signature (`minisign_verify::{PublicKey, Signature}`, L23) and returns
  the bytes; `install(bytes)` is separate (L686, L757). Download-now,
  install-later is the supported shape.
- macOS `install_inner` (L1294-1388): extract the tar to a tempdir, `rename`
  the current `.app` into a backup tempdir, `rename` the new one into place,
  `touch`. Needs no relaunch to *complete*; the running process keeps
  running from its unlinked inode.
- Config requires `plugins.updater.pubkey` in `tauri.conf.json`;
  `endpoints` may be set in code (`UpdaterBuilder::endpoints`, L216).
- Capability: the webview gets **no** `updater:*` permission. Jarvis exposed
  `updater:allow-check` (`capabilities/default.json:19`); here Rust does
  everything and the webview only receives an event.

### 2.2 theuth

- Version: `[workspace.package] version`, read as `CARGO_PKG_VERSION`
  (`rpc/src/api.rs:724`, `t --version` `main.rs:23`, Info.plist via Tauri).
- Desktop quit: tray/⌘Q → `request_quit` emits `quit-requested`
  (`desktop/src/lib.rs:328-335`) → webview runs RPC `shutdown` if it owns
  the daemon → `quit_ack` → `app.exit(0)` → `RunEvent::Exit` →
  `Launch::shutdown()` closes the pty (`lib.rs:31-36`, `launch.rs:80-82,
  293-297`). Logout/shutdown reach the same `RunEvent::Exit`.
- Sidecar detection: `THEUTH_LAUNCH=1` is set by **both** the desktop
  (25 §0.3) and the service units (`service.rs:303`), so it does not
  identify the bundle. `current_exe()` under `*.app/Contents/MacOS/` does.
- Service restart policy already exists: launchd `KeepAlive.SuccessfulExit
  = false` (`service.rs:278-281`), systemd `Restart=on-failure`
  (`service.rs:303`). A non-zero exit is restarted; a zero exit is not.
- Attached frontends: `rpc::bridge` holds one `broadcast::Sender<Event>`
  (`bridge.rs:50-52`); every RPC subscription and the link feed own a
  `Receiver`, so `tx.receiver_count() == 0` means no frontend is listening.
- TUI header: `Model::header` composes `model · effort · ctx · cache` plus
  `approvals_tag()`, `link_tag()`, `nodes_tag()` (`tui/src/model.rs:3908-
  3978`), rendered DarkGrey/Yellow/Red by `Level` (`ui.rs:367-371`).
  Transcript notice: `Entry::Info(String)` (`model.rs:675`, DarkGrey at
  `ui.rs:1147`), inserted above a streaming tail with `interject`
  (`model.rs:2146`).
- Desktop Settings: `AppSidebar.tsx:99-103` renders the `Settings` button
  with the lucide `Settings` icon; `General.tsx:171` shows `theuth
  ${info.version}` in a `Row`.
- Deps already in the tree: `reqwest 0.13` (rustls), `semver 1.0.28`
  (transitive), `flate2 1`, `tempfile` (transitive). New: `tar` (the
  updater plugin pulls it anyway), `minisign-verify` (~500 lines, no deps;
  the plugin's own verifier), `tauri-plugin-updater 2`.
- Packaging: `scripts/package-macos.sh` builds `target/release/bundle/
  macos/theuth.app`, codesigns inside-out after Tauri bundles (L60-65), then
  `hdiutil` → `theuth_<ver>_$(uname -m).dmg` (L53, L75). `just install` copies
  `target/release/t` to `$CARGO_HOME/bin/theuth` (`justfile:35-38`).
- Linux rename semantics: writing into a running executable fails with
  `ETXTBSY`; renaming a new file over its path does not (the old inode
  stays mapped). §3.2 step 3 writes `<exe>.new` and renames for exactly
  this reason; the same code is correct on both OSes.
- Homebrew: a formula installs to `$(brew --prefix)/Cellar/theuth/<ver>/bin/
  theuth` with a symlink at `$(brew --prefix)/bin/theuth` (`/opt/homebrew` on
  arm64 macOS, `/home/linuxbrew/.linuxbrew` on Linux); `current_exe()`
  resolves the symlink, so `/Cellar/theuth/` in the path identifies the
  install. `brew upgrade <formula>` runs `brew update` first unless
  `HOMEBREW_NO_AUTO_UPDATE=1`; that auto-update is rate-limited by
  `HOMEBREW_AUTO_UPDATE_SECS` (default 24 h). A cask with `auto_updates
  true` is skipped by `brew upgrade` unless `--greedy`. A formula may
  carry `on_macos` / `on_linux` blocks with separate `url`/`sha256`, and
  `on_arm` / `on_intel` inside them. A tap is a git repository named
  `homebrew-<tap>`; `brew tap basedatum/tap` clones
  `github.com/basedatum/homebrew-tap`.

## 3. Shape

### 3.1 Bucket and manifest

Same endpoint, new bucket: `https://s3.rwx.dev/theuth/releases/`. Policy is
jarvis's `s3/jarvis-bucket-policy.json` with the ARN changed: anonymous
`s3:GetObject` on `arn:aws:s3:::theuth/releases/*`, nothing else anonymous.

```
releases/<ver>/theuth_<ver>_macos-aarch64.dmg              # first install (Finder, cask)
releases/<ver>/theuth_<ver>_macos-aarch64.app.tar.gz       # desktop payload: theuth.app
releases/<ver>/theuth_<ver>_macos-aarch64.app.tar.gz.sig
releases/<ver>/theuth_<ver>_macos-aarch64.tar.gz           # CLI payload: one file, `theuth` (formula, self-update)
releases/<ver>/theuth_<ver>_macos-aarch64.tar.gz.sig
releases/<ver>/theuth_<ver>_linux-x86_64.tar.gz            # CLI payload
releases/<ver>/theuth_<ver>_linux-x86_64.tar.gz.sig
releases/<ver>/theuth_<ver>_linux-aarch64.tar.gz           # CLI payload
releases/<ver>/theuth_<ver>_linux-aarch64.tar.gz.sig
releases/<ver>/SHA256SUMS                               # for the formula/cask stanzas and humans
releases/latest.json                                    # the only mutable object
staging/<ver>/theuth_<ver>_linux-*.tar.gz                  # unsigned, from the Linux build hosts (§5.1); not public
```

`<os>-<arch>` is `std::env::consts::{OS, ARCH}`: `macos-aarch64`,
`linux-x86_64`, `linux-aarch64`. The Tauri key keeps Tauri's spelling
(`darwin-aarch64`) because the plugin computes it; nothing else uses it.

```json
{
  "version": "0.2.0",
  "platforms": {
    "darwin-aarch64":    { "url": "https://s3.rwx.dev/theuth/releases/0.2.0/theuth_0.2.0_macos-aarch64.app.tar.gz", "signature": "<.sig file, verbatim>" },
    "cli-macos-aarch64": { "url": "https://s3.rwx.dev/theuth/releases/0.2.0/theuth_0.2.0_macos-aarch64.tar.gz",     "signature": "<.sig file, verbatim>" },
    "cli-linux-x86_64":  { "url": "https://s3.rwx.dev/theuth/releases/0.2.0/theuth_0.2.0_linux-x86_64.tar.gz",      "signature": "<.sig file, verbatim>" },
    "cli-linux-aarch64": { "url": "https://s3.rwx.dev/theuth/releases/0.2.0/theuth_0.2.0_linux-aarch64.tar.gz",     "signature": "<.sig file, verbatim>" }
  }
}
```

`notes` and `pub_date` are optional in the plugin and nothing here displays
them; omitted. Objects under `<ver>/` are immutable (never re-uploaded);
`latest.json` is written last, after §5.2's checks, so a half-finished
release is invisible.

### 3.2 `crates/update` — the harness updates `theuth`

Plugin `UpdatePlugin`, actor registered as `"update"`, ~150 lines.

```rust
pub const URL: &str = "https://s3.rwx.dev/theuth/releases/latest.json";
pub const PUBKEY: &str = include_str!("../pubkey.txt");   // minisign .pub, base64 (Tauri's encoding)

#[derive(Clone, Serialize, Deserialize, JsonSchema)]      // wire-visible (00 §3)
pub struct UpdateReady { pub version: String, pub applied: bool }

pub struct Status;                                          // ask → Option<UpdateReady>
```

Start: read `update` section (§7); if `!enabled`, or `cfg!(debug_assertions)`,
or `current_exe()` is inside `*.app/Contents/MacOS/` (the desktop owns that
file, §3.3), spawn nothing and log why at `debug`. Otherwise spawn one task
selecting on `rt.shutdown` (the `schedule` tick pattern, `schedule/src/lib.rs:1123-1135`):

```
sleep FIRST_CHECK
loop {
  match check_and_apply().await {
    Ok(Some(ready)) => { publish(ready.clone()); state = Some(ready) }
    Ok(None)        => {}
    Err(e)          => debug!("update: {e}")
  }
  sleep EVERY + jitter
}
```

`check_and_apply`:

0. Where am I? `current_exe()` canonicalized: `/Cellar/theuth/` in the path →
   the Homebrew branch (step H below) replaces steps 2–3. Otherwise the
   file path is what gets replaced.
1. `GET URL` (reqwest, 30 s timeout, `User-Agent: theuth/<ver>`), parse
   `{version, platforms}`. `semver::Version` compare per §1.7; not newer →
   `Ok(None)`. Key `cli-{OS}-{ARCH}` absent → `Ok(None)` (a platform we do
   not publish for is not an error).
2. Download the tarball to `<data_dir>/updates/theuth_<ver>.tar.gz.part`
   (streamed to disk, not to memory — a headless box may be small).
   Verify: `PublicKey::from_base64(PUBKEY)?.verify(&bytes, &Signature::decode(&sig)?, false)`.
   Failure → delete, `Err`.
3. Extract the single entry `theuth` to `<exe>.new` beside `current_exe()`,
   `chmod 755`, `rename(<exe>.new, <exe>)`. Same directory ⇒ same
   filesystem ⇒ atomic; the running process is unaffected on both OSes
   (§2.2 `ETXTBSY`). Delete the tarball. `Ok(Some(UpdateReady { version,
   applied: true }))`.
4. If the directory is not writable (a `/usr/local/bin` owned by root,
   a read-only mount): skip 2–3, `Ok(Some(UpdateReady { version, applied:
   false }))` and one `info!` naming the path. The tag then reads
   `0.2.0 available` and `sudo t --update` (below) is the manual path.

H. Homebrew: run
   `HOMEBREW_AUTO_UPDATE_SECS=0 HOMEBREW_NO_ENV_HINTS=1 HOMEBREW_NO_INSTALL_CLEANUP=1 brew upgrade --formula theuth`
   with `brew` resolved from `PATH` (the process came from a login shell,
   18 §2, so the prefix is there), stdout/stderr to the log at `debug`,
   10 min timeout, at most once per manifest version. Exit 0 and
   `brew list --versions theuth` naming the manifest version → `Ok(Some(
   UpdateReady { version, applied: true }))`; the Cellar now holds the new
   file and the symlink points at it, the running process keeps its inode.
   Non-zero (brew's lock held by an interactive `brew`, the tap not yet
   bumped because §5 is mid-flight, no network) → `Err`, retried next tick.
   The manifest is still the oracle for *whether* to try: brew is only
   invoked when S3 says a newer version exists, so an idle install costs
   one GET per tick, never a `brew update`. `t --update` follows the same
   branch and prints brew's summary line.

State is one `Option<UpdateReady>`, in memory. After a restart the new
binary runs and there is nothing to remember. A staged-but-not-applied
version is rediscovered by the next tick.

**`t --update`** (clap flag beside `--install-service`): runs
`check_and_apply` once in the foreground, prints one line (`theuth 0.1.0 →
0.2.0` / `up to date` / the error) and exits. For `sudo t --update`, for
CI images, and for the check in §12. It is the same function; there is no
second implementation.

Also `run_once(cfg, data_dir)` is the function `--update` calls; the
plugin's tick calls the same.

### 3.3 `crates/desktop` — the app updates `theuth.app`

`tauri-plugin-updater` registered in `desktop::configure`; `tauri.conf.json`
gains `plugins.updater.pubkey` (same string as `pubkey.txt`; §5.2 checks
they match) and `createUpdaterArtifacts: false` (the tarball is made after
codesign, §5.1). Endpoint set in code from one const; no `endpoints` in
the JSON.

```rust
struct Pending(Mutex<Option<(Update, Vec<u8>)>>);   // managed state

// setup(): the same schedule as §3.2
tauri::async_runtime::spawn(async move {
    sleep(FIRST_CHECK).await;
    loop {
        if let Ok(Some(u)) = app.updater_builder().endpoints(vec![URL])?.build()?.check().await
            && same_major(&u)
            && let Ok(bytes) = u.download(|_, _| {}, || {}).await   // minisign-verified
        {
            app.emit("update-ready", &u.version)?;
            *pending.lock() = Some((u, bytes));
        }
        sleep(EVERY + jitter()).await;
    }
});

// run(): RunEvent::Exit, after Launch::shutdown()
if let Some((u, bytes)) = app.state::<Pending>().0.lock().take()
    && let Err(e) = u.install(&bytes) { error!("update: install: {e}") }
```

Order in `RunEvent::Exit`: `Launch::shutdown()` first (the sidecar it
spawned gets SIGHUP and exits — it was running from the old bundle, which
is fine, its inode survives the rename), then `install`. A reused terminal
daemon (18 §3) is not touched; it updates its own file via §3.2. Install is
an extract of a ~30 MB tarball plus two renames: well inside the time
macOS gives a terminating app at logout.

Bytes stay in memory between download and quit (≈30 MB thin `.app`; the
same choice jarvis made, `updater.rs:634-770`). Rejected: spooling to
disk and re-verifying at quit — a second verification path for a 30 MB
allocation that lives in an accessory app the user quits by closing the
lid at most.

ponytail: the plugin's macOS install (§2.1) keeps the old bundle only in
a tempdir that is dropped on return; a failure between its two renames
loses `/Applications/theuth.app`. The DMG on S3 is the recovery. If this
fires once, jarvis's `ditto` backup (`updater.rs:286-303`) is the upgrade.

### 3.4 A daemon under a service manager

`--install-service` daemons run for months and have no quit. After step 3
of §3.2 succeeds, and only when (a) `std::os::unix::process::parent_id() == 1` (launchd) or
`INVOCATION_ID` is set (systemd), and (b) `rpc.events` `receiver_count()
== 0` (nobody attached; the desktop reuses such a daemon only on the same
box and would otherwise be counted), and (c) no turn has started in the
last `IDLE_BEFORE_EXIT`: `std::process::exit(75)`. `EX_TEMPFAIL` is
non-zero, so `KeepAlive.SuccessfulExit=false` / `Restart=on-failure`
restart the daemon on the new binary; the exit reason is one `info!`
line in `logs/service.log`. Conditions re-checked each tick; a daemon
that is always attached updates at the next reboot, which is the
documented ceiling.

A desktop-spawned daemon never meets (a) (its parent is `theuth-desktop`),
so 18 §3 holds: it is never restarted from inside.

### 3.5 Homebrew: `basedatum/tap`

Repository `github.com/basedatum/homebrew-tap`, two files, both written by
`release-s3.sh` (§5.3) — nobody edits them by hand.

`Formula/theuth.rb` — a binary formula, no build step, both OSes:

```ruby
class Theuth < Formula
  desc "Harness that does things for the user"
  homepage "https://github.com/skymoore/theuth"
  version "0.2.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://s3.rwx.dev/theuth/releases/0.2.0/theuth_0.2.0_macos-aarch64.tar.gz"
      sha256 "<SHA256SUMS>"
    end
  end
  on_linux do
    on_arm do
      url "https://s3.rwx.dev/theuth/releases/0.2.0/theuth_0.2.0_linux-aarch64.tar.gz"
      sha256 "<SHA256SUMS>"
    end
    on_intel do
      url "https://s3.rwx.dev/theuth/releases/0.2.0/theuth_0.2.0_linux-x86_64.tar.gz"
      sha256 "<SHA256SUMS>"
    end
  end

  def install
    bin.install "t"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/t --version")
  end
end
```

`Casks/theuth.rb` — the `.app`, macOS arm64:

```ruby
cask "theuth" do
  version "0.2.0"
  sha256 "<SHA256SUMS of the DMG>"
  url "https://s3.rwx.dev/theuth/releases/#{version}/theuth_#{version}_macos-aarch64.dmg"
  name "theuth"
  desc "Desktop app for the theuth harness"
  homepage "https://github.com/skymoore/theuth"
  depends_on arch: :arm64
  depends_on macos: ">= :monterey"
  auto_updates true
  app "theuth.app"
  zap trash: "~/Library/Application Support/io.theuth.mac"
end
```

`brew install basedatum/tap/theuth` and `brew install --cask basedatum/tap/theuth`.
The formula and cask share the name deliberately (the product has one
name, 00 §1.1); brew disambiguates on `--cask` / `--formula`. `zap` names
only the window-state dir (25 §5): `~/.theuth` is the user's and survives an
uninstall.

What each does at update time:
- Formula: §3.2 step H upgrades it from inside the harness; a user's own
  `brew upgrade` finds it already current. sha256 in the formula is the
  bucket's `SHA256SUMS` line; the tarball is the same minisign-signed
  object §3.2 verifies, so a Homebrew install and a self-update install the
  identical bytes.
- Cask: `auto_updates true`; `tauri-plugin-updater` swaps the bundle
  (§3.3); `brew upgrade` ignores it, `brew upgrade --greedy` reinstalls the
  version the cask names, which is never older than what the app installed
  because §5.3 bumps the cask in the same run that writes `latest.json`.

Rejected: `brew livecheck` / `brew bump-formula-pr` — homebrew-core
tooling for a repo we own; a `sed` and a commit is the whole job.
Rejected: Homebrew as the *only* CLI channel — a headless box without brew
and a `just install` developer need §3.2 anyway.

## 4. Surfaces

Everything here is a fact that was already true when it is shown; nothing
asks for a decision.

### 4.1 TUI

- `Model.update: Option<UpdateReady>` set from the bus event. Header gains
  `update_tag()` after `nodes_tag()`: `  ·  0.2.0 ready` when `applied`,
  `  ·  0.2.0 available` otherwise. Same DarkGrey as its neighbours; it
  does not change `Level`.
- At the moment of the event, one `interject(Entry::Info(..))`:
  `theuth 0.2.0 is installed; it runs the next time you start theuth.` or
  `theuth 0.2.0 is available; \`t --update\` installs it (…/bin is not
  writable).` Once per version, in the current session only.
- No `/update` command. The binary is already replaced; the command would
  be "quit theuth". Rejected: exec-into-new-binary from the TUI — `exec` skips
  destructors, so raw mode and the alternate screen would need manual
  teardown; not worth it for a restart the user does daily.

### 4.2 Desktop

- Rust emits `update-ready { version }`; a Zustand slice holds it.
- `AppSidebar.tsx` Settings button: a 6 px dot (the `Dot` from
  `HarnessSwitcher.tsx`, muted colour) next to the label while a version
  is pending. That is the "subtle indicator".
- Settings › General, the existing `Harness` row (`General.tsx:171`) gets
  a sibling `Row`: label `App`, value `theuth 0.1.0 · 0.2.0 installs when you
  quit`. No button. Quit is one tray click away and already exists.
- Tray menu: unchanged. Rejected: jarvis's `Update to vX…` item and the
  post-relaunch success toast.
- `notifyIfHidden` (`notify.ts`) is **not** called. A system notification
  is a pop-up.

### 4.3 RPC

- `UpdateReady` joins the `forward!` list in `bridge.rs` so any client
  (a phone over link, the desktop attached to a terminal daemon) can show
  the daemon's own state if it wants; the desktop does not in v0.
- `Api::update_status() -> Option<UpdateReady>`: `ask` on `"update"`.
  Settings › General shows it under the `Harness` row as `0.2.0 ready ·
  restarts with the daemon` when the daemon is not the bundled sidecar.
  Added to `export.rs`, `bindings/methods.json`, `api.ts` per 25 §2.1.

## 5. Publishing

### 5.1 Artifacts

`scripts/release-s3.sh` (ported from jarvis's, `JARVIS_`→`THEUTH_`, python
heredoc → `jq`), driven by `just release`. The macOS side runs on the
release Mac (Developer ID, notarytool — 25 §5); the Linux side runs where
the CPU is and hands its result to the same script through the bucket.

```
on each Linux build host (once per arch, any time before `just release`):
  just stage-linux         # docker build → theuth_<ver>_linux-<arch>.tar.gz → s3://theuth/staging/<ver>/  (unsigned)

on the release Mac:
just release:
  package-macos            # theuth.app + DMG, Developer ID, hardened runtime (exists)
  notarize-macos           # notarytool submit + staple the DMG (exists)
  package-update-macos     # new: .app.tar.gz + macOS CLI tar.gz
  release-s3               # new: pull staging/<ver>/*, sign everything, gates, upload <ver>/…, verify, latest.json, tap bump
  tag-release              # new: annotated v<ver>, pushed
```

**Linux build.** `deploy/update/Dockerfile` builds `theuth` (not the relay)
natively for the host's arch: `FROM ubuntu:22.04` (glibc 2.35 — the oldest
we support; covers Ubuntu 22.04+, Debian 12+, Fedora 36+, and Homebrew on
Linux, which requires ≥ 2.13), Rust from `rust-toolchain.toml`, the C
toolchain obscura/btls/libsignal need (`clang`, `cmake`, `llvm`, `perl`,
`pkg-config`, `libssl-dev` if anything still wants it, X11/xcb headers for
enigo/xcap), `cargo build --release -p theuth`, output the one file. `just
stage-linux` = `docker build --output type=local` + tar + `aws s3 cp` to
`staging/<ver>/`. Run on an arm64 Mac for `linux-aarch64` (Docker Desktop,
native) and on an x86_64 Linux box for `linux-x86_64` — never under QEMU
(V8 under emulation is hours). No GitHub Actions: the repo has none, jarvis
published from none (`ci.yml:129-130`), and the private key never leaves
the release Mac this way. The staging prefix is not covered by the public
policy; `release-s3.sh` deletes it after publishing.

`package-update-macos.sh`:

```sh
cd "$BUNDLE_DIR"
COPYFILE_DISABLE=1 tar --no-xattrs -czf "theuth_${VER}_${KEY}.app.tar.gz" theuth.app
COPYFILE_DISABLE=1 tar --no-xattrs -czf "theuth_${VER}_${KEY}.tar.gz" -C theuth.app/Contents/MacOS theuth
```

`KEY="$(uname -s | tr A-Z a-z | sed s/darwin/macos/)-$(uname -m | sed s/arm64/aarch64/)"`
— the one place the shell spelling is mapped to Rust's (the Linux
Dockerfile uses the same line). The macOS CLI tarball is the sidecar file
itself: already Developer-ID signed with identifier `io.theuth.mac.t` and
stapled by the `.app`'s notarization. Signing of all five tarballs happens
in `release-s3.sh`, in one place, after the Linux ones are pulled from
staging:

```sh
for f in dist/*.tar.gz; do cargo tauri signer sign -k "$THEUTH_UPDATE_KEY" -p "$THEUTH_UPDATE_KEY_PASSWORD" "$f"; done
(cd dist && shasum -a 256 *.tar.gz *.dmg > SHA256SUMS)
```

Signing key: `cargo tauri signer generate -w ~/.tauri/theuth.key` once, new
for this product (jarvis's key stays jarvis's). Public half committed as
`crates/update/pubkey.txt` and in `tauri.conf.json`. Private half and
password in `.env.signing` (`THEUTH_UPDATE_KEY`, `THEUTH_UPDATE_KEY_PASSWORD`),
loaded by the existing `scripts/load-signing-env.sh`. The key pitfalls
jarvis documented (`package-updater-macos.sh:50-58`: KDF=none keys, double
base64) are avoided by generating with `-w` and a password once and
never touching the encoding again.

### 5.2 Gates (the part jarvis designed and skipped)

`release-s3.sh` aborts before any upload when:

1. `VERSION` from `Cargo.toml` ≠ `CFBundleShortVersionString` of the `.app`;
2. `codesign -dv theuth.app 2>&1 | grep -q 'Authority=Developer ID'` fails
   (ad-hoc build) — unless `THEUTH_RELEASE_PRERELEASE=1`, which writes only
   `latest-prerelease.json`, never `latest.json`, and never bumps the tap;
3. `spctl --assess --type execute theuth.app` fails (not notarized);
4. `grep -q "$(cat crates/update/pubkey.txt)" crates/desktop/tauri.conf.json`
   fails (the two pubkey copies diverged);
5. `s3://theuth/releases/<ver>/` already exists (immutability);
6. `staging/<ver>/` lacks either Linux tarball — unless
   `THEUTH_RELEASE_LINUX=skip`, which publishes a manifest without the
   `cli-linux-*` keys and a formula without the `on_linux` block (Linux
   installs then no-op on that version per §3.2 step 1; a hotfix that only
   matters on macOS need not wait for a Linux box);
7. each staged Linux tarball, extracted in a clean `docker run --rm
   ubuntu:22.04` of its arch, does not print `VERSION` for `t --version`
   (missing shared library, wrong glibc — caught here, not on a user's
   box).

After uploading `<ver>/*`, and before writing `latest.json`: download each
tarball and `.sig` back through the **public** URL and verify with
`minisign -V` (or `cargo tauri signer verify`) against `pubkey.txt`. Only
then `aws s3 cp latest.json`. What the client will fetch has been fetched
and checked once, by the publisher.

### 5.3 Tap bump

Last step, after `latest.json` is live (so a formula never points at an
object that is not yet readable): `git clone --depth 1
git@github.com:basedatum/homebrew-tap`, write `Formula/theuth.rb` and
`Casks/theuth.rb` from the two templates in §3.5 with `VERSION` and the
`SHA256SUMS` lines filled in, `brew audit --strict --formula Formula/theuth.rb`
and `brew audit --cask Casks/theuth.rb` (syntax, url reachability, sha —
brew is on the release Mac), `git commit -m "theuth ${VERSION}"`, `git push`.
A failed audit leaves the tap untouched and the release otherwise
complete: Homebrew installs simply see the new version one run later.

### 5.4 Env

`AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY`, `THEUTH_S3_ENDPOINT`
(`https://s3.rwx.dev`), `THEUTH_S3_BUCKET` (`theuth`), `THEUTH_S3_PREFIX`
(`releases`), `THEUTH_S3_PUBLIC_BASE_URL` (derived), `THEUTH_UPDATE_KEY[_PASSWORD]`,
`THEUTH_RELEASE_PRERELEASE`, `THEUTH_RELEASE_LINUX` (`require` | `skip`),
`THEUTH_TAP_REPO` (`git@github.com:basedatum/homebrew-tap`), `THEUTH_S3_DRY_RUN`.
Same rustfs compatibility flags as jarvis (`AWS_S3_FORCE_PATH_STYLE=true`,
checksums `when_required`, `AWS_EC2_METADATA_DISABLED=true`). All from
`.env.signing` (`.env.signing.example` documents them).

## 6. Security (28)

- Manifest and payloads travel over TLS to a host we control; the
  signature, not TLS, is the trust root. A compromised bucket can serve
  bytes but cannot sign them. The pubkey ships in the binary.
- Version pinning by `<ver>/` immutability: a manifest can only point at
  a version that was published whole, once.
- No downgrade (§1.7): a manifest replayed from the past is a no-op.
- The webview has no updater capability; the harness exposes only a
  read-only `update_status`. Nothing the model can call touches this.
- `update.url` is a config value: a user who redirects it to their own
  bucket still needs artifacts signed by our key. Overriding the pubkey
  is not a config option.
- Key rotation: ship a binary that trusts both keys, wait one release
  cycle, drop the old. Not built; the day it is needed `PUBKEY` becomes a
  two-element array and `verify` tries each.

## 7. Config

```yaml
update:
  enabled: true            # check, verify, replace; false = the harness never touches the network for this
  url: https://s3.rwx.dev/theuth/releases/latest.json   # scratch bucket for the checks in §12
```

Two keys, both with defaults; `THEUTH_UPDATE__ENABLED`, `THEUTH_UPDATE__URL`
via figment (00 §9). The desktop reads neither (it is not a plugin, 25
§0.1); `THEUTH_UPDATE_URL` in its environment overrides the const for §12.
No channel, no interval, no "ask me" mode: constants live in code
(36 §1). Channel comes back as `update.channel: prerelease` → `latest-
prerelease.json` the day someone wants it; the script already writes it.

## 8. Data on disk

| path | owner | content | delete → |
|---|---|---|---|
| `<data_dir>/updates/theuth_<ver>.tar.gz.part` | harness | in-flight download | nothing; re-downloaded |
| `<exe>.new` | harness | extracted binary, exists for milliseconds | nothing |
| `crates/update/pubkey.txt` | repo | minisign public key, base64 | builds fail to verify anything |
| `~/.tauri/theuth.key` | release machine | private key | no more releases |
| `github.com/basedatum/homebrew-tap` `Formula/theuth.rb`, `Casks/theuth.rb` | release script | generated per release (§5.3) | `brew install` breaks until the next release regenerates them |
| `deploy/update/Dockerfile` | repo | Linux build recipe (§5.1) | no Linux tarballs |

No state file, no phase file, no `localStorage`.

## 9. Numbers

`FIRST_CHECK = 60 s` (never on the startup path), `EVERY = 6 h`, jitter
`0..5 min` (a fleet of daemons does not hit the bucket on the hour),
`HTTP_TIMEOUT = 30 s`, `IDLE_BEFORE_EXIT = 10 min` (§3.4). In code, not
config (36 §1). Jarvis's 12 h was chosen for a check that ended in a toast;
6 h for one that ends in nothing is cheap: a 400-byte GET.

## 10. Rejected

- **Port jarvis's shell updater.** Its 966 lines are daemon hunting,
  phases, relaunch and recovery for a flow the user drives by clicking.
  With no click there is no flow.
- **`self_update` / `self-replace` crates.** `rename` over the binary is
  the whole job on unix. `self-replace` becomes the answer if Windows
  arrives (running exes cannot be renamed there).
- **Harness restarts itself into the new binary** (`CommandExt::exec`):
  drops every RPC connection; the desktop shows "connection lost" —
  a toast. The restart the user will do anyway costs nothing.
- **Desktop restarts itself after install** (jarvis `relaunch_app`,
  `updater.rs:194-249`): an app that quits and reappears is an
  interruption.
- **Notes / changelog in the UI.** Nowhere to show them without a
  surface the user has to dismiss. The git tag and GitHub release hold
  them.
- **A daemon-pushed update event as the desktop's trigger.** See §1.2.
- **Tauri's `createUpdaterArtifacts`.** Snapshots the bundle before
  `package-macos.sh` codesigns it (25 §5).
- **ETag / `If-None-Match` on the manifest.** 400 bytes, four times a
  day.
- **`brew` as the only Linux/macOS CLI channel, or self-replace inside
  the Cellar.** §1.10.
- **Cross-compiling Linux from the Mac / QEMU.** V8 and BoringSSL under
  emulation take hours; a native container on a box of the right arch
  takes minutes. `staging/` is the hand-off.
- **Homebrew bottles.** A bottle is a build artifact brew produces from a
  formula's `install`; ours has no build. The tarball *is* the bottle.

## 11. Not built, and when

- **Intel macOS**: not wanted. 25 §5 is arm64-only; `on_intel` is absent
  from the formula's `on_macos` block on purpose, so `brew install` on an
  Intel Mac fails at resolution instead of at runtime.
- **Linux desktop**: 25 is macOS-only. The CLI/service path (§3.2, §3.4)
  is the Linux story.
- **Prerelease channel**: `update.channel`, one line in the plugin, one in
  the desktop; the script already writes the file.
- **Dual pubkey** for rotation: §6.
- **Other package managers** (apt/dnf repos, AUR, nix): each is a
  "leave the file alone, tell the manager" branch like step H, plus a
  publish step. Add when one has a user; `cargo install` is covered by
  §3.2 already (`~/.cargo/bin` is writable).

## 12. Build steps, each with its check

1. **Bucket.** Create `theuth`, apply the policy, run `test-s3-rustfs.sh`
   against it: authed PUT 200, anonymous GET 200 under `releases/`,
   anonymous GET 403 under `staging/`, anonymous PUT 403.
2. **Key + script.** `cargo tauri signer generate`, `pubkey.txt`,
   `package-update-macos.sh`, `release-s3.sh` with §5.2 gates. Check: an
   ad-hoc build aborts at gate 2; a `--prerelease` run writes only
   `latest-prerelease.json` and leaves the tap alone; a repeat of the same
   version aborts at gate 5; missing staging aborts at gate 6 and
   `THEUTH_RELEASE_LINUX=skip` produces a manifest with two keys.
3. **Linux build.** `just stage-linux` on the arm64 Mac and on an x86_64
   box; gate 7 runs `t --version` in a clean `ubuntu:22.04` of each arch
   and prints `VERSION`. On a `debian:12` and a `fedora:40` container the
   same tarball's `t --version` also works (glibc floor holds).
4. **`crates/update` + `t --update`.** Publish `0.1.1` to a scratch
   prefix; `THEUTH_UPDATE__URL=<scratch> t --update` on a `0.1.0`
   `~/.cargo/bin/theuth` prints `theuth 0.1.0 → 0.1.1`; `t --version` prints
   `0.1.1`; `codesign --verify ~/.cargo/bin/theuth` passes. Same on a Linux
   box with `~/.cargo/bin/theuth`, and with `/usr/local/bin/theuth` owned by
   root: `available`, then `sudo t --update` → `0.1.1`. Flip one byte in
   the uploaded tarball: `t --update` prints the verify error, the binary
   is unchanged. Publish `0.0.9`: `up to date`. Publish `1.0.0`: `up to
   date` (§1.7).
5. **Homebrew.** `brew install basedatum/tap/theuth` at `0.1.0` on macOS and
   on Linux; publish `0.1.1` (tap bumped by §5.3): `t --update` prints
   brew's `theuth 0.1.0 -> 0.1.1`, `brew list --versions theuth` → `0.1.1`,
   `readlink -f $(which theuth)` is under `Cellar/theuth/0.1.1/`, and `~/.cargo/
   bin` was not touched. With an interactive `brew install something`
   holding the lock: `t --update` reports brew's lock error, nothing
   changes, the next tick succeeds. `brew install --cask basedatum/tap/theuth`
   installs `/Applications/theuth.app`; after step 7 below `brew upgrade`
   prints nothing for it and `brew upgrade --greedy` finds it current.
6. **TUI.** Run `theuth` `0.1.0` with the scratch URL and `FIRST_CHECK`
   left at 60 s: within ~60 s the header shows `· 0.1.1 ready` and one
   grey line lands in the transcript; nothing else changes; `Ctrl-C`,
   `t --version` → `0.1.1`.
7. **Sidecar guard.** Launch `theuth.app`, wait 2 min, `codesign --verify
   --deep --strict /Applications/theuth.app` passes: the harness did not
   touch `Contents/MacOS/t`.
8. **Desktop.** `theuth.app` `0.1.0` in `/Applications`, `THEUTH_UPDATE_URL=
   <scratch>`: dot on Settings within ~60 s, Settings › General shows
   `0.1.1 installs when you quit`. Tray → Quit. `PlistBuddy -c 'Print
   :CFBundleShortVersionString'` → `0.1.1`; relaunch works, `info()` from
   the new sidecar reports `0.1.1`; `spctl --assess` on the installed
   `.app` passes (the tarball carried the notarized bundle).
9. **Service.** `t --install-service` on a `0.1.0` (launchd on the Mac,
   systemd on the Linux box), publish `0.1.1`, no client attached: within
   `EVERY + IDLE_BEFORE_EXIT` `logs/service.log` shows `update: 0.1.1 in
   place, exiting for restart` and the daemon comes back as `0.1.1`. With
   the desktop attached: the binary is replaced, the daemon stays up,
   `update_status` reports `ready`.
10. **Silence.** Steps 6–9 with the network unplugged: no output anywhere,
    one `debug!` line per tick in the log.

## 13. Amendments to earlier docs (same commit)

- 25 §5 "Updater. Not in v0" → "Updater: 39. `tauri-plugin-updater` for
  the bundle, installed at quit; the harness updates its own binary."
- 25 §5 "Version lock": add "both `~/.cargo/bin/theuth` and the sidecar
  update themselves to the same `latest.json`; skew lasts until the
  next restart of whichever is behind."
- 18 §3 "No auto-restart loop in v0": add "still true for a
  desktop-spawned daemon; a service-managed daemon exits `75` after an
  update when nothing is attached (39 §3.4)."
- 00 §10 workspace table: `update` crate, deps `reqwest`, `semver`,
  `minisign-verify`, `flate2`, `tar`. 00 §12 phases: one row.
- 00 §2: a sentence after "How a server starts it": "how any of them
  stays current: 39."
- 25 §5 "Targets … arm64 host-only": add "the CLI also ships for
  `linux-x86_64` / `linux-aarch64` (39 §5.1); the `.app` stays arm64
  macOS."
- 32 §5.1 (`--install-service`): add "the unit's non-zero-exit restart
  policy is what 39 §3.4 relies on; do not change it to restart on any
  exit."
- README install section: `brew install basedatum/tap/theuth`, `brew install
  --cask basedatum/tap/theuth`, and the tarball URL pattern, replacing `just
  install` as the first instruction for users who are not building.

## 14. Status

Built 2026-09-17, one commit. Code: `crates/update` (plugin, `run_once`,
3 tests), `crates/theuth` (`--update`, plugin vector, `DEFAULT_CONFIG`),
`crates/rpc` (`update_status`, `UpdateReady` bridged and exported,
`SetAttached` told at start), `crates/tui` (header tag, one `Entry::Info`,
1 test), `crates/desktop` (`src/update.rs`: `tauri-plugin-updater`,
`Pending`, install in `RunEvent::Exit` after `Launch::shutdown()`;
`ui/src/lib/update.ts`, sidebar dot, two Settings rows, `update_status` in
`api.ts`), `scripts/{release-s3,package-update-macos,stage-linux,
tag-release,test-s3-rustfs}.sh`, `scripts/tap/*.tmpl`,
`scripts/s3/theuth-bucket-policy.json`, `deploy/update/Dockerfile`, `justfile`
recipes, `.env.signing.example`. Operator steps: `docs/update-setup.md`.

As built, two deviations from the text above:
- **Desktop writability guard** (§3.3): before downloading, the app checks
  `access(W_OK)` on `theuth.app` and its parent. The plugin's macOS install
  falls back to an AppleScript "with administrator privileges" dialog when
  the rename hits `PermissionDenied` — a pop-up, and one that would block
  `RunEvent::Exit` — so a root-owned `/Applications/theuth.app` is left
  alone (debug log) rather than risk it.
- **Key encoding**: `cargo tauri signer` writes `.pub` and `.sig` as base64
  of the minisign text; `pubkey.txt` and the manifest `signature` are
  therefore base64-decoded first, exactly as the plugin does. The
  harness verifies while streaming (`verify_stream`): the tarball never
  sits in memory.

Exercised (gates: `cargo check/clippy -D warnings/test`, `just ts-desktop`,
`bun run check`, `bun run test` all green):
- §12 step 4 on this Mac with a throwaway key baked into `pubkey.txt` and
  a debug `theuth` against a local `http.server` manifest: `0.0.9` → `up to
  date (0.1.0)`; `1.0.0` → `up to date`; a tarball with one flipped byte →
  `update: signature does not verify …`, exit 1, binary unchanged,
  `updates/` empty; `0.1.1` → `theuth 0.1.0 → 0.1.1`, the file at `current_exe`
  is the new payload with mode 755, no `.new` left; the same in a
  `chmod 555` directory → `theuth 0.1.1 available; … is not writable — run
  `sudo t --update``, exit 0, binary unchanged.
- TUI header/transcript behaviour by unit test (`model.rs`
  `update_ready_tags_header_and_lines_once`); extract-and-rename and the
  version rule by unit test in `crates/update`.
- `bash -n` on every script; `just --summary` lists the five recipes;
  the tap templates render byte-for-byte to §3.5 (ReleaseScripts' throwaway).

Not exercised (needs the bucket, the key, a Linux host or a signed app —
`docs/update-setup.md` §7 is the checklist): §12 steps 1–3, 5–10 — the
plugin's 6 h tick end to end, the Homebrew branch (`brew upgrade` against
a real tap), the service `exit(75)` restart, the desktop download +
install at quit, the Linux Docker build (apt list is a best guess from
`cargo tree`; first `just stage-linux` will say), `release-s3.sh` against
rustfs, `brew audit` on the rendered files.
