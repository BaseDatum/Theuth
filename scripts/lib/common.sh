# shellcheck shell=bash
# Sourced helpers shared by the scripts.
need() { command -v "$1" >/dev/null 2>&1 || { echo "error: $1 required${2:+ ($2)}" >&2; exit 1; }; }
size_of() { stat -c %s "$1" 2>/dev/null || stat -f %z "$1"; } # GNU first: BSD `-f %z` is a valid *filesystem* query on GNU and returns garbage
