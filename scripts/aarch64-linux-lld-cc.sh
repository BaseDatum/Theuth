#!/usr/bin/env bash
# Linker driver for aarch64-unknown-linux-gnu (.cargo/config.toml [target.aarch64-unknown-linux-gnu] linker).
#
# GNU ld (bfd) needs >11 GB to link theuth's debug test binaries on arm64 (the rpc `prompts` test was
# OOM-killed on a 15 GiB VM, "collect2: fatal error: ld terminated with signal 9"). rustc already ships
# LLD for every target (rust-lld + the gcc-ld/ld.lld shim), so this hands cc that linker: a few hundred
# MB and a few seconds for the same binary. x86_64-unknown-linux-gnu uses rust-lld by default since
# rustc 1.90; aarch64 does not yet, and `-C link-self-contained=+linker` is still nightly-only.
set -euo pipefail
gcc_ld="$(rustc --print sysroot)/lib/rustlib/aarch64-unknown-linux-gnu/bin/gcc-ld"
exec "${CC:-cc}" -fuse-ld=lld -B"$gcc_ld" "$@"
