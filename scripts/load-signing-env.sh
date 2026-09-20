# shellcheck shell=bash
# Source from packaging scripts:  source "$(dirname "$0")/load-signing-env.sh"
# Loads repo-root .env.signing if present. Does not override variables already set.
_theuth_signing_env="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/.env.signing"
if [[ -f "$_theuth_signing_env" ]]; then
  set -a
  # shellcheck disable=SC1090
  source "$_theuth_signing_env"
  set +a
fi
unset _theuth_signing_env
