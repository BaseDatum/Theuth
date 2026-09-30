#!/usr/bin/env bash
# deploy/vm/bootstrap.sh embeds the four units as heredocs; fail if they drift
# from deploy/vm/units/*.service (comment lines ignored). Also `bash -n` the script.
set -euo pipefail
cd "$(dirname "$0")/.."
bash -n deploy/vm/bootstrap.sh
rc=0
for u in xvnc desktop novnc theuth; do
  if ! diff -u \
      <(grep -v '^#' "deploy/vm/units/$u.service") \
      <(awk "/^cat > \/etc\/systemd\/system\/$u.service/{f=1;next} /^EOF\$/{f=0} f" deploy/vm/bootstrap.sh); then
    echo "drift: deploy/vm/units/$u.service vs bootstrap.sh" >&2; rc=1
  fi
done
[ $rc -eq 0 ] && echo "vm units in sync"
exit $rc
