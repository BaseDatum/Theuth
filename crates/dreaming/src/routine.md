---
every: 30m
when: dreaming.py window={{window}} idle={{idle}} heavy_concepts={{heavy_concepts}} heavy_sessions={{heavy_sessions}}{{engine_arg}}
fire: while_true
cooldown: 3h
mode: skip
memory: clear
runtime: clear
quiet: true
approvals: false
run_timeout: {{run_timeout}}
budget: {{budget}}
max_runs: 100000
cwd: {{cwd}}
{{model_line}}---
Load the `dreaming` skill and run tonight's phases in order. Read `dreaming_ledger()` first; stop at the budget; write the journal last.
