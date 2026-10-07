#!/bin/bash
# `swift test` with the Command Line Tools only: there is no XCTest, and SwiftPM does not add the CLT's
# Swift Testing framework or its interop dylib by itself (Swift 6.3.2 / macOS 26.6.2, spike SA machine).
# Wrap in build-lock on shared machines: `BUILD_LABEL=130-w1 build-lock ./test.sh`.
set -euo pipefail
cd "$(dirname "$0")"
DEV="$(xcode-select -p)/Library/Developer"
F="$DEV/Frameworks"
L="$DEV/usr/lib"
exec swift test --disable-xctest --enable-swift-testing \
  -Xswiftc -F"$F" -Xlinker -F"$F" -Xlinker -rpath -Xlinker "$F" -Xlinker -rpath -Xlinker "$L" "$@"
