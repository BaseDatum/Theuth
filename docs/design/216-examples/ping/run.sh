#!/usr/bin/env bash
# ping: one status line every 10 s (design 216 §13.2). It reads stdin only to learn when to stop.
host=${PING_HOST:-1.1.1.1}
while true; do
  ms=$(ping -c 1 "$host" 2>/dev/null | sed -n 's/.*time=\([0-9.]*\).*/\1/p')
  if [ -n "$ms" ]; then dot=ok; tip="$host $ms ms"; else dot=error; tip="$host unreachable"; fi
  printf '{"jsonrpc":"2.0","method":"status","params":{"placement":"ping","dot":"%s","tooltip":"%s"}}\n' "$dot" "$tip"
  if read -r -t 10 line; then
    case $line in *'"method":"shutdown"'*) exit 0 ;; esac
  elif [ $? -le 128 ]; then
    exit 0   # stdin closed: the host is gone
  fi
done
