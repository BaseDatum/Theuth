#!/bin/bash
# presses: a status-strip clock, no SDK (`t plugin new --raw --lang sh`; the shape of design 216 §13.2).
# One status line every 10 s. It reads stdin only to learn when to stop. macOS /bin/bash 3.2 and bash 4+.
read -r hello || exit 0            # the first line is always `hello`; EOF: the host is gone
while true; do
  now=$(date +%H:%M)
  printf '{"jsonrpc":"2.0","method":"status","params":{"placement":"presses","dot":"ok","label":"%s","tooltip":"%s"}}\n' \
    "$now" "Local time $now"
  start=$SECONDS
  if read -r -t 10 line; then
    case $line in *'"method":"shutdown"'*) exit 0 ;; esac
  else
    rc=$?
    # A timeout is >128 on bash 4+ but 1 on bash 3.2, the same as EOF. A timeout returns only
    # after the full 10 s, so a failure that came sooner is EOF: the host is gone.
    if [ "$rc" -le 128 ] && [ $((SECONDS - start)) -lt 9 ]; then exit 0; fi
  fi
done
