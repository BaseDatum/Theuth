#!/bin/bash
# Conformance fixture (not a template): every SDK implements this same behavior; see ../../host.py.
. "$(dirname "$0")/theuth-plugin.sh"

paint() {
  n=$(theuth_state_int n)
  theuth_status dot=ok badge="$n" label="$n"
  theuth_ui text="$n presses" button="Press:inc"
}
start() { paint; theuth_ping && theuth_log pong now_ms:="$(theuth_get .now_ms "$THEUTH_RESULT")"; }

theuth_on_start start
theuth_on_render paint # without it, render re-runs on_start (which pings here)
theuth_action inc 'theuth_state_add n 1; paint'
theuth_action deadlock 'theuth_request ping "{}" && echo "$THEUTH_RESULT"'
theuth_action call 'theuth_call echo "{\"x\":1}" && echo "$THEUTH_RESULT"'
theuth_action denied 'theuth_call shell || echo "{\"code\":$(theuth_get .code "$THEUTH_ERROR")}"'
theuth_action boom 'echo boom; return 1'
theuth_action extras '
  theuth_emit pressed n:="$(theuth_state_int n)"
  theuth_notify title=Hi body=there
  theuth_invalidate
  theuth_send custom/note a:=1
  echo "$THEUTH_ARGS"'
theuth_tool presses 'theuth_state_int n'
theuth_tool fail 'echo kaput; return 1'
theuth_command count 'echo "n=$(theuth_state_int n)"'
theuth_on conformance.tick 'theuth_log tick k="$(theuth_get .data.data.k)"'
theuth_on 'agent.events#TurnFinished' 'theuth_log turn stop_reason="$(theuth_get .data.stop_reason)"'
theuth_on_request custom/echo 'echo "$THEUTH_PARAMS"'
theuth_on_granted 'theuth_log granted placements:="$(theuth_get ".grants.placements|length")"'
theuth_on_shutdown 'theuth_log bye'
theuth_run
