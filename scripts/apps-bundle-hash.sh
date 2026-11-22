#!/usr/bin/env bash
# Print the committed in-process bundles' input/output hashes (crates/js-runtime/js/apps.sha256 format).
# Needs only bash + shasum: `just apps-bundle-check`, scripts/app-parser-env.sh (`check_apps_bundle`) and
# scripts/release-s3.sh compare it with the committed crates/js-runtime/js/apps.sha256 (review M1/m5).
#
#   inputs <sha256>          OpenUI bundle: exactly the package-local files it was built from
#                            (crates/js-runtime/js/theuth-apps-inproc.inputs, written by scripts/build-inproc.ts)
#                            + generated/*, patches/*, package.json, bun.lock, tsconfig.json, its build scripts
#   bundle <sha256>          crates/js-runtime/js/theuth-apps-inproc.js
#   mermaid-inputs <sha256>  mermaid bundle: theuth-mermaid-inproc.inputs + patches/*, package.json, bun.lock,
#                            tsconfig.json, its build scripts
#   mermaid-bundle <sha256>  crates/js-runtime/js/theuth-mermaid-inproc.js
#
# Run from the repository root. Each hash is over the sorted per-file `shasum -a 256` lines.
set -euo pipefail
js="$PWD/crates/js-runtime/js"
cd packages/apps
hash_of() { LC_ALL=C sort -u | while read -r f; do shasum -a 256 "$f"; done | shasum -a 256 | cut -d' ' -f1; }
inputs=$({ cat "$js/theuth-apps-inproc.inputs"; find generated patches -type f; printf '%s\n' package.json bun.lock tsconfig.json scripts/build-inproc.ts scripts/bundle-inputs.ts; } | hash_of)
bundle=$(shasum -a 256 "$js/theuth-apps-inproc.js" | cut -d' ' -f1)
minputs=$({ cat "$js/theuth-mermaid-inproc.inputs"; find patches -type f; printf '%s\n' package.json bun.lock tsconfig.json scripts/build-inproc-mermaid.ts scripts/bundle-inputs.ts; } | hash_of)
mbundle=$(shasum -a 256 "$js/theuth-mermaid-inproc.js" | cut -d' ' -f1)
printf 'inputs %s\nbundle %s\nmermaid-inputs %s\nmermaid-bundle %s\n' "$inputs" "$bundle" "$minputs" "$mbundle"
