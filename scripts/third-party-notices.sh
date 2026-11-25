#!/usr/bin/env bash
# 221 B10: regenerate THIRD-PARTY-NOTICES from Cargo.lock (all features, so Bitwarden is in).
# Run before each release and commit the result. Needs cargo-about
# (`cargo binstall cargo-about`, or the prebuilt tarball from its GitHub releases).
set -euo pipefail
cd "$(dirname "$0")/.."
cargo about generate --locked --all-features -c about.toml \
  -o THIRD-PARTY-NOTICES scripts/third-party-notices.hbs
echo "THIRD-PARTY-NOTICES: $(wc -l < THIRD-PARTY-NOTICES) lines"
