#!/bin/bash
# ping: one status line every 10 s (design 216 §13.2). Runs on macOS /bin/bash 3.2 and on bash 4+.
# It reads stdin only to learn when to stop.
host=${PING_HOST:-1.1.1.1}
read -r hello || exit 0            # the first line is always `hello`; EOF: the host is gone
while true; do
  ms=$(ping -c 1 "$host" 2>/dev/null | sed -n 's/.*time=\([0-9.]*\).*/\1/p')
  if [ -n "$ms" ]; then dot=ok; label="$ms ms"; else dot=error; label=down; fi
  printf '{"jsonrpc":"2.0","method":"status","params":{"placement":"ping","dot":"%s","label":"%s","tooltip":"%s %s"}}\n' \
    "$dot" "$label" "$host" "$label"
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
