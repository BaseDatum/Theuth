#!/bin/bash
# Design 134 §5.7: the portable half of the Windows companion — Theuth.Companion.Rpc, Theuth.Companion.Core and their
# xUnit suites — on the Mac (or any machine with the .NET 10 SDK). The WinUI app (src/Theuth.Companion) needs Windows
# and is built on build-B with build.ps1; the solution filter leaves it out.
set -euo pipefail
cd "$(dirname "$0")"
if [ -n "${DOTNET:-}" ]; then
  :
elif [ -x "$HOME/.dotnet/dotnet" ]; then
  DOTNET="$HOME/.dotnet/dotnet"
  export DOTNET_ROOT="${DOTNET_ROOT:-$HOME/.dotnet}"
elif command -v dotnet >/dev/null 2>&1; then
  DOTNET=dotnet
else
  echo "test.sh: no .NET SDK — install .NET 10 to ~/.dotnet (dotnet-install.sh) or set DOTNET" >&2
  exit 1
fi
export DOTNET_CLI_TELEMETRY_OPTOUT=1 DOTNET_NOLOGO=1 DOTNET_SKIP_FIRST_TIME_EXPERIENCE=1
exec "$DOTNET" test Theuth.Companion.Portable.slnf "$@"
