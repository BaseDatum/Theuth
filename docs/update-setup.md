# Update: operator setup

Status: checklist · 2026-09-18 · companion to design/39-update.md and 46-apps.md §2.1

Everything a human configures outside the repo before the first release
works. Once per product, not per release, except §6. Env values live in
`.env.signing` (gitignored, loaded by `scripts/load-signing-env.sh`);
`.env.signing.example` names each one.

## 1. Bucket

Same rustfs endpoint as jarvis (`https://s3.rwx.dev`), new bucket `theuth`.
Anonymous `GetObject` on `releases/*` only; `staging/*` and every write
stay authenticated (39 §3.1).

1. Create the bucket and apply the policy (admin credentials, once). The CORS
   rule lets theuth.io read `latest.json` in the browser so its "download for
   macOS" button follows the newest release instead of the site's build-time
   version:

   ```sh
   aws --endpoint-url https://s3.rwx.dev --region us-east-1 s3 mb s3://theuth
   aws --endpoint-url https://s3.rwx.dev --region us-east-1 \
     s3api put-bucket-policy --bucket theuth \
     --policy file://scripts/s3/theuth-bucket-policy.json
   aws --endpoint-url https://s3.rwx.dev --region us-east-1 \
     s3api put-bucket-cors --bucket theuth \
     --cors-configuration file://scripts/s3/theuth-bucket-cors.json
   ```

2. Create an access key for the release Mac (rustfs console or
   `rustfs admin user add`), scoped to `theuth` if the deployment supports
   it. Put it in `.env.signing`:

   ```sh
   AWS_ACCESS_KEY_ID=...
   AWS_SECRET_ACCESS_KEY=...
   THEUTH_S3_ENDPOINT=https://s3.rwx.dev     # default; stated for clarity
   THEUTH_S3_BUCKET=theuth                      # default
   THEUTH_S3_PREFIX=releases                 # default
   ```

3. Probe:

   ```sh
   ./scripts/test-s3-rustfs.sh
   ```

   Expected, in order: authenticated `PUT` under `releases/` → 200;
   anonymous `GET` of that object → 200; anonymous `GET` under
   `staging/` → 403; anonymous `PUT` → 403; probe object deleted. Any
   other line is a policy or credential problem — fix before §6, the
   release script assumes all four.

## 2. Signing key

One minisign key for this product; jarvis's key stays jarvis's.

1. Generate with a password (never `--ci`, never an empty password — the
   KDF-less key `tauri signer` produces that way cannot sign later,
   39 §5.1):

   ```sh
   cargo tauri signer generate -w ~/.tauri/theuth.key
   ```

   Enter a password when asked. Output: `~/.tauri/theuth.key` (private,
   encrypted) and `~/.tauri/theuth.key.pub` (public, already Tauri's base64
   form — one line).

2. Commit the public half in both places, byte-identical, single line:

   ```sh
   tr -d '\n' < ~/.tauri/theuth.key.pub > crates/update/pubkey.txt
   # crates/desktop/tauri.conf.json → plugins.updater.pubkey: the same string
   grep -q "$(cat crates/update/pubkey.txt)" crates/desktop/tauri.conf.json && echo match
   git add crates/update/pubkey.txt crates/desktop/tauri.conf.json
   git commit -m "update: signing pubkey"
   ```

   Release gate 4 (39 §5.2) refuses to publish if the two copies differ.
   Do not pipe the `.pub` through `base64` again.

3. Private half and password in `.env.signing`:

   ```sh
   THEUTH_UPDATE_KEY=~/.tauri/theuth.key        # path (preferred) or the file's base64 contents
   THEUTH_UPDATE_KEY_PASSWORD=...
   ```

   `scripts/release-s3.sh` accepts either form: a path is mapped to
   `TAURI_SIGNING_PRIVATE_KEY_PATH`, base64 contents to
   `TAURI_SIGNING_PRIVATE_KEY`.

4. Back up `~/.tauri/theuth.key` and the password somewhere that is not the
   release Mac (password manager, offline). Losing the private key means
   every installed client can never update again: the pubkey is compiled
   into every `theuth` and every `theuth.app`, and a new key requires a manual
   reinstall of every install (DMG or `brew reinstall`). See §8.

## 3. Apple

Already in place from 25 §5; listed because release gates 2–3 depend on it.

1. `.env.signing` has `THEUTH_SIGN_IDENTITY` (Developer ID Application, SHA-1
   hash preferred) and the `THEUTH_NOTARY_*` values from
   `.env.signing.example`; `xcrun notarytool store-credentials theuth-notary`
   has been run once.
2. The release Mac has `bun` (`brew install oven-sh/bun/bun`): `just
   package-macos` builds the app parser helper from `packages/apps` with
   `bun build --compile` into a standalone executable (46 §2.1), named
   `theuth-app-parser-<hash>` after the committed
   `packages/apps/generated/library-hash.txt`, and signs it with
   `scripts/entitlements-app-parser.plist` (JIT, for Bun's runtime). Bun is
   a build tool only; nothing shipped needs Node or Bun.
3. `just package-macos && just notarize-macos` produces a `.app` for which

   ```sh
   codesign -dvv target/release/bundle/macos/theuth.app 2>&1 | grep 'Authority=Developer ID'
   codesign -dvv target/release/bundle/macos/theuth.app/Contents/MacOS/theuth-app-parser-* 2>&1 | grep 'Authority=Developer ID'
   spctl --assess --type execute target/release/bundle/macos/theuth.app
   ```

   all pass. An ad-hoc build (`THEUTH_SIGN_IDENTITY` unset), an unsigned helper
   or one without the JIT entitlement stops `release-s3.sh` at gate 2 and an
   un-notarized bundle at gate 3; only `THEUTH_RELEASE_PRERELEASE=1` bypasses
   them, and it writes `latest-prerelease.json`, never `latest.json`, and
   never bumps the tap.

## 4. Homebrew tap

Public repo `basedatum/homebrew-tap` (`brew tap basedatum/tap`). `Formula/`
and `Casks/` stay empty until `release-s3.sh` writes `theuth.rb` (39 §5.3).
Nobody edits those files by hand.

1. The release Mac must push over ssh without a prompt:

   ```sh
   ssh -T git@github.com          # "Hi skymoore! You've successfully authenticated"
   ```

   `THEUTH_TAP_REPO` defaults to `git@github.com:basedatum/homebrew-tap`;
   set it in `.env.signing` only for a different remote.

2. Confirm the tap resolves (empty until the first release):

   ```sh
   brew tap basedatum/tap
   brew tap-info basedatum/tap
   ```


## 5. Linux build hosts

Linux CLI tarballs are built natively per arch in Docker, never under QEMU
(39 §5.1), and handed to the release Mac through `s3://theuth/staging/<ver>/`.
Each holds two files: `theuth` and `theuth-app-parser-<hash>`, the helper built in
the same image by a pinned Bun (`deploy/update/Dockerfile`, `BUN_VERSION` +
its two `SHASUMS256` sums — bump all three together). No host needs Bun.

1. `linux-aarch64`: Docker Desktop on the arm64 release Mac (or any arm64
   Mac with the repo checked out).
2. `linux-x86_64`: an x86_64 Linux box with Docker and the repo checked
   out at the release commit.
3. Both hosts need the AWS variables from §1 (upload to `staging/` is
   authenticated) — copy `.env.signing` minus the signing key, or export
   `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` / `THEUTH_S3_ENDPOINT`.
4. On each, before `just release` on the Mac:

   ```sh
   just stage-linux           # docker build → theuth + theuth-app-parser-<hash> → theuth_<ver>_linux-<arch>.tar.gz → s3://theuth/staging/<ver>/
   ```

   `THEUTH_LINUX_ARCH` overrides the detected arch. The script refuses an image
   whose helper hash is not the checkout's. Gate 6 aborts the release if
   either tarball is missing; gate 7 runs, from each staged tarball in a
   clean `ubuntu:24.04` container, `t --version`, `t --app-parser-hash`
   (must print the shipped helper's hash) and the helper's own `--version`;
   gate 8 lists every CLI tarball and requires exactly those two entries.
5. macOS-only release (hotfix, no Linux box at hand):
   `THEUTH_RELEASE_LINUX=skip just release` publishes a manifest without the
   `cli-linux-*` keys and a formula without the `on_linux` block; Linux
   installs skip that version and pick up the next one.

## 6. First release walkthrough

1. On each Linux host at the release commit: `just stage-linux` (§5).
2. On the release Mac, bump `[workspace.package] version` in `Cargo.toml`
   (semver; a config/schema break is a major and is not silently
   applied, 39 §1.7), commit.
3. Run the pipeline:

   ```sh
   just release
   # = package-macos → notarize-macos → package-update-macos → release-s3 → tag-release
   ```

4. In `release-s3.sh`'s summary look for: all eight gates passed
   (39 §5.2; gate 8 = every CLI tarball is exactly `theuth` +
   `theuth-app-parser-<hash>` and the macOS pair answers `--version` /
   `--app-parser-hash` with that hash); five tarballs signed; `SHA256SUMS`
   written; each object re-downloaded through the public URL and verified
   (`minisign -V` if installed, else sha256 against `SHA256SUMS`);
   `latest.json` uploaded last; `staging/<ver>/` deleted; tap commit
   `theuth <ver>` pushed; `brew audit` clean for formula and cask; tag `v<ver>`
   pushed. The manifest shape is unchanged: one URL per platform, the CLI
   tarball now carries both files.
5. Verify from a machine that is not the release Mac:

   ```sh
   curl -s https://s3.rwx.dev/theuth/releases/latest.json | jq .
   #  .version == the new version; .platforms has darwin-aarch64, cli-macos-aarch64,
   #  cli-linux-x86_64, cli-linux-aarch64 (the last two absent with THEUTH_RELEASE_LINUX=skip)
   brew install basedatum/tap/theuth && t --version
   ls "$(dirname "$(readlink -f "$(which theuth)")")"/theuth-app-parser-"$(t --app-parser-hash)"   # the helper the formula installed beside it
   brew install --cask basedatum/tap/theuth && ls /Applications/theuth.app/Contents/MacOS/theuth-app-parser-*
   ```
6. Speech models, once (and again only when a checkpoint changes): with the
   four checkpoints in `~/.theuth/models/` (pocket-tts, parakeet-tdt,
   nemotron-streaming-en, pyannote), `just release-models` uploads the files
   each loader opens to `releases/models/<model>/<file>` and writes
   `releases/models/models.json` (`{files: [{path, size, sha256}]}`) last.
   The desktop app reads that manifest at start and fetches whatever is
   missing under `<data_dir>/models/` (`crates/desktop/src/models.rs`;
   `THEUTH_MODELS_URL` points at a scratch manifest). Check:
   `curl -s https://s3.rwx.dev/theuth/releases/models/models.json | jq '.files|length'` → 11.

## 7. Verification checklist (39 §12)

Tick each on the first release and after any change to `crates/update`,
`crates/desktop` updater code, or the scripts.

- [ ] 1 Bucket: `test-s3-rustfs.sh` — authed PUT 200, anonymous GET 200
      under `releases/`, anonymous GET 403 under `staging/`, anonymous PUT 403.
- [ ] 2 Key + script: an ad-hoc build, an unsigned helper or one without
      the JIT entitlement aborts at gate 2; a `THEUTH_RELEASE_PRERELEASE=1` run
      writes only `latest-prerelease.json` and leaves the tap alone;
      repeating a version aborts at gate 5; missing staging aborts at gate
      6; a stale `dist/` tarball from another library hash aborts at gate 8;
      `THEUTH_RELEASE_LINUX=skip` yields a two-key manifest.
- [ ] 3 Linux build: `just stage-linux` on both hosts; gate 7 prints the
      version, `wants=<hash>` and `theuth-app-parser <hash>` from
      `ubuntu:24.04`; the same tarball's `t --version` and
      `theuth-app-parser-<hash> --version` work in `debian:13` and `fedora:40`.
- [ ] 4 `t --update`: `0.1.1` on a scratch prefix, `THEUTH_UPDATE__URL=<scratch>
      t --update` on a `0.1.0` `~/.cargo/bin/theuth` prints `theuth 0.1.0 → 0.1.1`,
      `t --version` → `0.1.1`, `codesign --verify` passes (macOS); same on
      Linux; root-owned `/usr/local/bin/theuth` → `available`, then `sudo theuth
      --update`; a flipped byte in the tarball → verify error, binary
      unchanged; `0.0.9` and `1.0.0` → `up to date`. Helper: after the
      update `~/.cargo/bin` holds the new `theuth-app-parser-<hash>` *and* the
      old one (never removed — a still-running `0.1.0` may spawn it), the
      helper was renamed in before `theuth`, and `t --app-parser-hash` names
      the file beside it. A payload whose `t --app-parser-hash` does not
      match its helper, or with any third entry, is refused with the binary
      unchanged; a payload whose helper hash is already installed keeps the
      existing file. `THEUTH_APP_PARSER=<path>` overrides the lookup in every
      build — a development/test hook, never set it in a service unit.
- [ ] 5 Homebrew: formula at `0.1.0` on macOS and Linux; publish `0.1.1`;
      `t --update` prints brew's summary, `brew list --versions theuth` →
      `0.1.1`, `readlink -f $(which theuth)` under `Cellar/theuth/0.1.1/` with
      `theuth-app-parser-<hash>` beside it (the formula asserts its `--version`),
      `~/.cargo/bin` untouched; with brew's lock held → brew's lock error,
      nothing changes, next tick succeeds; cask installs
      `/Applications/theuth.app`; after step 8 `brew upgrade` is silent and
      `brew upgrade --greedy` finds it current.
- [ ] 6 TUI: `0.1.0` with the scratch URL; within ~60 s the header shows
      `· 0.1.1 ready` and one grey transcript line; nothing else; quit,
      `t --version` → `0.1.1`.
- [ ] 7 Sidecar guard: launch `theuth.app`, wait 2 min, `codesign --verify
      --deep --strict /Applications/theuth.app` passes.
- [ ] 8 Desktop: `theuth.app` `0.1.0`, `THEUTH_UPDATE_URL=<scratch>`; Settings
      dot within ~60 s; General shows `0.1.1 installs when you quit`; Tray →
      Quit; `PlistBuddy -c 'Print :CFBundleShortVersionString'` → `0.1.1`;
      relaunch works, `info()` reports `0.1.1`; `spctl --assess` passes.
- [ ] 9 Service: `--install-service` on `0.1.0` (launchd and systemd),
      publish `0.1.1`, nothing attached: within `EVERY + IDLE_BEFORE_EXIT`
      `service.log` shows the exit line and the daemon returns as `0.1.1`;
      with the desktop attached the binary is replaced, the daemon stays up,
      `update_status` → `ready`.
- [ ] 10 Silence: steps 6–9 with the network unplugged — no output
      anywhere, one `debug!` line per tick.

## 8. Rotation and recovery

- **Roll back a bad release.** Publish a higher version. Clients never
  downgrade (39 §1.7), so `0.2.1` that is `0.2.0`'s code is the rollback.
  Bump `Cargo.toml`, `just release`.
- **Pull a bad release before anyone has it.** Objects under
  `releases/<ver>/` are immutable and the previous version's are still
  there; only `latest.json` moves. Rebuild the 39 §3.1 JSON for the
  previous version from its `.sig` files and re-point `latest.json`:

  ```sh
  source scripts/load-signing-env.sh
  P=<prev>; B=https://s3.rwx.dev/theuth/releases/$P
  aws --endpoint-url https://s3.rwx.dev --region us-east-1 s3 sync s3://theuth/releases/$P /tmp/theuth-$P --exclude '*' --include '*.sig'
  jq -n --arg v "$P" \
    --arg d "$(cat /tmp/theuth-$P/theuth_${P}_macos-aarch64.app.tar.gz.sig)" \
    --arg m "$(cat /tmp/theuth-$P/theuth_${P}_macos-aarch64.tar.gz.sig)" \
    --arg x "$(cat /tmp/theuth-$P/theuth_${P}_linux-x86_64.tar.gz.sig)" \
    --arg a "$(cat /tmp/theuth-$P/theuth_${P}_linux-aarch64.tar.gz.sig)" \
    --arg b "$B" '{version:$v, platforms:{
      "darwin-aarch64":    {url:"\($b)/theuth_\($v)_macos-aarch64.app.tar.gz", signature:$d},
      "cli-macos-aarch64": {url:"\($b)/theuth_\($v)_macos-aarch64.tar.gz",     signature:$m},
      "cli-linux-x86_64":  {url:"\($b)/theuth_\($v)_linux-x86_64.tar.gz",      signature:$x},
      "cli-linux-aarch64": {url:"\($b)/theuth_\($v)_linux-aarch64.tar.gz",     signature:$a}}}' > /tmp/latest.json
  aws --endpoint-url https://s3.rwx.dev --region us-east-1 \
    s3 cp /tmp/latest.json s3://theuth/releases/latest.json --content-type application/json
  ```

  Drop the `cli-linux-*` entries if `<prev>` was a `THEUTH_RELEASE_LINUX=skip`
  release. Clients that already installed the bad version stay on it until
  a higher version ships — this only stops the spread. The tap still names
  the bad version; revert its commit by hand or let the next release fix it.
- **Key rotation** (39 §6). Not built; the plan: ship a release whose
  binaries trust both the old and the new key (`PUBKEY` becomes a
  two-element list and `verify` tries each; `tauri.conf.json` follows
  suit), signed with the old key. Wait one release cycle so every
  self-updating install has it. Then sign with the new key only and drop
  the old pubkey. A compromised key skips the wait: revoke by shipping the
  dual-key build immediately, signed with the old key one last time, and
  rotate `.env.signing`.
- **Lost key, no backup.** No release can be signed that existing installs
  accept. Generate a new key (§2), commit the new pubkey, publish a release
  the old clients will ignore, and tell users to reinstall once: DMG from
  `releases/<ver>/`, or `brew reinstall theuth` / `brew reinstall --cask theuth`
  (the tap points at the new objects). Every install after that
  self-updates again. This is the scenario §2 step 4 exists to prevent.
