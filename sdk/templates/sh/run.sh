#!/bin/bash
. "$(dirname "$0")/theuth-plugin.sh"
paint() { n=$(theuth_state_int n); theuth_status dot=ok badge="$n" label="$n"; theuth_ui text="$n presses" button="Press:inc"; }
theuth_action inc 'theuth_state_add n 1; paint'
theuth_tool presses 'theuth_state_int n'
theuth_on_start paint
theuth_run
