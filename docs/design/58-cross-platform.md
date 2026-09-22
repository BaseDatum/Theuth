# 58 — Cross-platform: one `main` builds `t` on macOS, Linux and Windows

## 0. Decisions in one screen

1. **Signal's encrypted SQLite store is non-negotiable on every platform.**
   The channel (09) keeps `presage-store-sqlite`, hence SQLCipher, on all
   three hosts; a platform-specific unencrypted fallback is rejected.
2. **SQLCipher's crypto provider is BoringSSL on every host; OpenSSL is not
   in the tree.** `vendor/libsqlite3-sys` (crates.io 0.36.0 + build.rs)
   compiles SQLCipher with `SQLCIPHER_CRYPTO_OPENSSL` against the headers
   of libsignal's `boring-sys` and emits no crypto link-lib of its own; the
   symbols come from the `libcrypto.a` boring-sys already links. This is the
   route Signal Desktop takes for its `better-sqlite3` fork. The Apple build
   no longer uses CommonCrypto: one provider, one behaviour, one set of bugs.
3. **Two BoringSSLs may be linked, never two `libcrypto` symbol sets.**
   libsignal's `boring-sys` (`links = "boringssl"`) owns the plain names;
   wreq's `btls-sys` (`links = "btls"`, under obscura's stealth stack) must
   prefix every export — C and Itanium-mangled C++ alike — as `btls_sys_*`.
4. **Forks are pinned and named in `Cargo.toml`**, each with why, what
   differs and when to drop it (00 §1.3): `skymoore/btls`
   `fix/prefix-cxx-elf`, `skymoore/boring` `theuth/cargo-include`.

## 1. Goal, and what is not one

`cargo build -p theuth` on a stock macOS, Ubuntu 24.04 and Windows host
produces a `t` that runs `--version` and opens the Signal store. Not a goal:
cross-compiling from one host to another, FIPS, or sharing a single
BoringSSL between libsignal and wreq (each pins its own).

## 2. Facts measured on the build hosts (2026-09-22)

### 2.1 Duplicate `bssl::*` symbols

On Ubuntu 24.04 the final link of `t` failed with `rust-lld: duplicate
symbol: bssl::…` ×20 between `libboring_sys` and `libbtls_sys`. `btls-sys`'s
`prefix-symbols` pass renamed C exports on ELF but skipped Itanium-mangled
C++ symbols. Fixed in `skymoore/btls` `fix/prefix-cxx-elf` (`fcf88a04`);
zero duplicates on the rebuild. Windows/COFF still skips prefixing — open.

### 2.2 Two BoringSSLs and SQLCipher

- crates.io `libsqlite3-sys 0.36.0` (feature `bundled-sqlcipher`, pulled by
  `presage-store-sqlite`) compiles SQLCipher 4.10 for **OpenSSL 3** and
  emits `-lcrypto`; on Apple it takes CommonCrypto instead, which is why
  macOS never showed the problem. Signal's `boring-sys` also emits
  `rustc-link-lib=static=crypto`; rustc collapses both into one `-lcrypto`,
  the linker resolves it inside boring-sys's `-L` dirs to BoringSSL's
  `libcrypto.a`, and SQLCipher's `EVP_MAC_fetch`, `EVP_MD_get_size`,
  `EVP_CIPHER_get_*` go undefined. On Windows the same crate demands
  `OPENSSL_DIR` and would hit the same clash.
- SQLCipher 4.10's `crypto_openssl.c` needs, beyond what BoringSSL has, only
  the OpenSSL-3 `EVP_MAC_*` / `OSSL_PARAM` HMAC surface. BoringSSL reports
  `OPENSSL_VERSION_NUMBER 0x1010107f`, so every other version branch in the
  provider is the 1.1 one and resolves. `sqlcipher/boringssl_evp_mac.h`
  (static functions, force-included via `sqlite3_boringssl.c`) implements
  that surface over `HMAC_CTX`; `sqlite3.c` is byte-identical to upstream.
- `boring-sys` emits no `cargo:include`; the fork adds the one `println!`
  so the vendored crate reads `DEP_BORINGSSL_INCLUDE`. The `[patch."https://
  github.com/signalapp/boring"]` entry redirects every `boring-sys 5.0.2` in
  the tree (libsignal's and ours) to the same fork commit, so there is still
  exactly one BoringSSL build behind `links = "boringssl"`.
- Precedent: Signal Desktop builds its `better-sqlite3` fork's SQLCipher
  against BoringSSL for the same reason (one crypto library shared with
  libsignal). The on-disk format is SQLCipher's, independent of provider:
  a store written by the OpenSSL-provider `sqlcipher` CLI opens in `t`'s
  build and vice versa (09 §2.3).
- `openssl-sys` remains in `Cargo.lock` only as a host build-dependency of
  `ort-sys`/`wavekat-vad` (via `ureq`'s native-tls on Linux); nothing links
  it into `t` on any target (`cargo tree -i openssl-sys -e normal --target
  <triple>` is empty for all three).
