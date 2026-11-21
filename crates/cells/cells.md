---
name: cells
description: Write user cells and runbooks the user runs with one press — load before presenting commands to run, a run sheet, or a note with cells.
---

# User cells: code the user runs with one press

A user cell is code you present and the user runs. You write it; theuth shows it with its folder, its shell and a Run button named after its consequence; the user presses; you get the result. You never run it.

## 1. What a user cell is

A closed fence in a runnable language whose body starts with `#|` option lines:

````text
```bash
#| name: preview-ir-prod1
#| cwd: ~/dr-t6/frp-integration/stacks/sa-frp-integration
#| expect: 3 to update
pulumi preview -s ir-prod1 --diff
```
````

- Runnable languages (first word of the info string): `bash`, `zsh`, `sh`, `pwsh`, `python`. Anything else is never a cell.
- Header: consecutive `#| key: value` lines from the first body line; a bare `#| key` means true. The header ends at the first other line; what follows is the code that runs. The header is never sent to the shell.
- A fence without a header is a snippet: the user reads and copies it, nothing runs.
- The same text works in a chat reply, in a note (`doc_insert`), in a notebook and in exported markdown.

Options (a closed set; an unknown key is a warning):

| option | value | meaning |
|---|---|---|
| `cwd` | `~/…` or an absolute path in chat; in a note `~/…` or relative to the note's `cells` default | the folder it runs in; required in chat; must exist on the machine that presses it |
| `name` | slug `[a-z0-9-]`, ≤ 48 | stable id; presenting the same name later replaces the older cell |
| `label` | text, keep it ≤ 40 | the Run button's text: name the consequence ("Apply to ir-prod1"); a longer one shows truncated |
| `danger` | bare | red outline, never in Run all, whole cell from its button only (no keyboard run); a line or selection ▶ asks the user to confirm first |
| `after` | a `name` in the same chat or note | advisory: arms when that cell's latest run exited 0 with every expect met, on its current code, on the machine that presses it; until then a press asks the user to confirm once and runs. Use sparingly |
| `expect` | text or `/regex/`, repeatable | must appear in the output; met or missed per line |
| `env` | `KEY=value`, repeatable | exported before the run; values may be `{{secret:NAME}}` |
| `python` | `monty`, `auto`, `.venv`, `uv` or an absolute path | which interpreter runs a python cell |
| `review` | bare | you are woken when this run (or the Run all pass holding it) ends |
| `private` | bare | you get exit and expect only, never the output |
| `timeout` | `30s`, `10m`, `2h` | Stop when it elapses, Kill 5 s later |

`on` is reserved (a sandbox or another engine, later); do not write it.

Note defaults: the first ```` ```cells ```` block of a note holds header lines only — `cwd` (a cell's relative `cwd` resolves against it), `shell` (the program for `bash`/`zsh`/`sh` cells, a program name such as `bash`, resolved on the machine that runs the cell; an absolute path only when every machine has it) and `python`. One per note.

## 2. When to present cells

Present cells instead of running anything yourself when:

| present cells | keep working with your tools |
|---|---|
| "prep it, don't apply", "don't run it", "production danger, just present me with the commands" | work the user asked you to do ("fix the build", "run the tests") |
| "give me the commands", "I'll run it", "present the commands", "show me what you'd run" | read-only checks you may run yourself (`kubectl get`, `pulumi stack ls`) |
| "step by step, I'll review each output", "go one step at a time" | a snippet inside an explanation ("the flag looks like this") — plain fence |
| "make me a run sheet", "write the runbook" | commands for another machine the user pastes elsewhere — plain fence |
| a step changes production or shared state you were not told to change, needs SSO/MFA, is interactive, or the user wants to watch it | — |

The mode is sticky: once asked, every changing step of that work stays a cell until the user lifts it ("go ahead, run it"). Read-only checks stay yours unless the user said hands off entirely — then even `get`/`list` become cells.

## 3. Writing rules

- `#| cwd:` on every cell: absolute or `~/…` in chat; in a note `~/…` or relative to the note's `cells` default, never one machine's absolute path (a note's cells run on whichever machine the reader presses them on). Never a `cd` line in place of `cwd`; never a `bash -c "…"` wrapper.
- One action per cell. Checks before changes: export or back up, preview, then apply.
- Explicit context on every command: `--profile`, `--context`, `-s <stack>`, `--region`. Never rely on the user's current default.
- Every changing step: `#| label:` naming the consequence in at most 40 characters and `#| danger`. An apply gets `#| after:` naming its preview — sparingly: it is advisory, the user can run any cell in any order. Only an apply: `after:` is not an ordering hint, so a read-only or non-changing step never gets one.
- `#| expect:` is text the tool really prints ("3 to update", "Exported to"); take it from the run sheet's "You should see". Plain text beats a regex.
- `#| review` when you promised to read the output. `#| private` when the output holds data you must not see. `#| timeout:` for anything that may hang.
- Secrets: `#| env: TOKEN={{secret:NAME}}` and the code uses `$TOKEN`. A placeholder in the code is an error.
- Names read like steps and carry the target: `export-ir-prod1`, `preview-ir-prod1`, `apply-ir-prod1`.
- No `--yes`, `-y`, `-auto-approve`, `--force` or `--skip-preview` on a danger cell unless the user asked: the tool's own prompt appears in the live terminal and the user answers it.
- A `bash` cell runs bash even when the login shell is zsh; write bash, not zsh-isms.

## 4. Interpreters

- `#| python: monty` for pure computation on the standard library: no packages, no network, no subprocess, no files outside the cwd. theuth checks Monty can run it when the reply ends.
- Anything else uses the project's interpreter: `auto` (nearest `.venv` → uv project → pyenv/conda → `python3` on the login PATH), `.venv`, `uv` or an absolute path.
- `interpreters(cwd)` lists what can run in a folder, with `why` and the one `auto` picks; read it before choosing. It answers for your own machine; a note may be run on another, so prefer `auto` or `.venv` over a path in a note.
- Keep one python interpreter per note: cells that name different ones run in different kernels and do not share variables.
- "Monty can't run this cell: line 1: no module named pandas — Monty runs the standard library only; use a local python" means the cell needs the project's interpreter: re-present it with `#| python: .venv` (or `auto`).

## 5. Chat or note

- Up to five one-off steps go in chat, in one reply, in run order.
- Longer or repeatable procedures go in a note (`doc_insert` the same fences) with one line in chat linking it.
- A run sheet as a note: a heading per step, one cell per action under it, "You should see: …" becomes `#| expect:`, a stop line ("if it doesn't say exit 0, stop") needs nothing — Run all stops at a failed step and before every danger cell. A ```` ```cells ```` block under the title holds the shared folder and shell.
- `doc_insert` refuses the whole insert when a cell does not validate and names each problem ("cell 2: cwd_missing …"); fix the cells and retry. Warnings (an unknown option, a long label, an `after` naming no cell) never refuse.

## 6. After a result

A finished run arrives as `<cell_result run="…" exit="…" expect="met 1/1" …>` with the cleaned output (head and tail; `cell_output(run, offset)` reads the cut middle).

- Check each expect line; report what matters in one or two lines (counts, the resource that changed, the error).
- Propose the next cell, or the corrected one under the same `name`. Re-present a cell only when it changes.
- `exit="lost"` means the engine restarted mid-run: the command may have half-applied. Check the real state first (a stack lock, a pending operation, the resource itself) with a read-only check or a cell; propose nothing that assumes it finished or did not.
- A `<cell_error>` lists cells the user cannot run: re-present each corrected cell with the same name, say nothing about the error.
- `cell_runs()` lists this chat's runs; `cell_runs(scope=<doc id>)` a note's.
- Never ask "shall I run it?" — the button is the question.

## 7. Anti-patterns

- Presenting cells for work the user asked you to do yourself, or asking "shall I run it?".
- Running the presented command through `shell`/`start` "to check", or typing into a cell's terminal.
- A cell per line of a script, or one 60-line cell doing five actions.
- `cd` lines instead of `#| cwd:`; relative paths in chat; one machine's absolute path (/Users/…, /opt/homebrew/…) in a note.
- A changing step without `label` and `danger`; `--yes`/`-auto-approve` on a danger cell.
- Regex soup in `expect`; expecting text the tool does not print.
- Re-presenting unchanged cells every turn (supersede only on change).
- Editing a note's checkboxes to report progress: a run is the record.
- Secrets in code; `bash -c "$(…)"` wrappers that hide the real script.

## 8. Worked examples

### IaC preview → apply in chat

User: "prep the preview and up for sa-frp-integration on ir-prod1 — don't apply, production". Reply:

`````markdown
Three steps. Run them in order; the apply arms after the preview matches.

```bash
#| name: export-ir-prod1
#| label: Export state
#| cwd: ~/dr-t6/frp-integration/stacks/sa-frp-integration
#| env: AWS_PROFILE=ir-prod1
#| expect: Exported to
pulumi stack export -s ir-prod1 --file /tmp/sa-frp-integration-ir-prod1-pre.json
echo "Exported to /tmp/sa-frp-integration-ir-prod1-pre.json"
```

```bash
#| name: preview-ir-prod1
#| label: Preview
#| cwd: ~/dr-t6/frp-integration/stacks/sa-frp-integration
#| env: AWS_PROFILE=ir-prod1
#| expect: 3 to update
pulumi preview -s ir-prod1 --refresh=false --diff
```

```bash
#| name: apply-ir-prod1
#| label: Apply to ir-prod1
#| danger
#| after: preview-ir-prod1
#| review
#| cwd: ~/dr-t6/frp-integration/stacks/sa-frp-integration
#| env: AWS_PROFILE=ir-prod1
#| expect: 3 updated
pulumi up -s ir-prod1 --refresh=false --diff
```
`````

After the preview's `<cell_result>`: "Preview matches: 3 updates (secret tags, policy replica ARN, role managed policy), nothing replaced or deleted. Apply when ready."

### A DR step-by-step run sheet in a note

`doc_insert` into a note "DR ir-prod1 failover", then one line in chat: "Run sheet: DR ir-prod1 failover (note)".

`````markdown
```cells
#| cwd: ~/dr-t6/frp-integration
#| shell: bash
```

## 1. Check replica lag

```bash
#| name: check-lag
#| env: AWS_PROFILE=ir-prod1
#| expect: ReplicaLag: 0
aws rds describe-db-instances --region us-east-1 --db-instance-identifier frp-replica --query 'DBInstances[0].StatusInfos' --output text
```

## 2. Promote replica

```bash
#| name: promote-replica
#| label: Promote frp-replica
#| danger
#| after: check-lag
#| review
#| env: AWS_PROFILE=ir-prod1
#| expect: modifying
aws rds promote-read-replica --region us-east-1 --db-instance-identifier frp-replica
```

## 3. Verify writer endpoint

```bash
#| name: verify-writer
#| env: AWS_PROFILE=ir-prod1
#| expect: available
aws rds describe-db-instances --region us-east-1 --db-instance-identifier frp-replica --query 'DBInstances[0].DBInstanceStatus' --output text
```
`````

### Data exploration

```python
#| name: sanity-sum
#| cwd: ~/Develop/costs
#| python: monty
costs = [120.5, 80.25, 42.0, 310.75, 9.5]
sum(costs), max(costs)
```

```python
#| name: load-costs
#| cwd: ~/Develop/costs
#| python: .venv
import pandas as pd
df = pd.read_csv("costs.csv")
df.groupby("service")["usd"].sum().sort_values(ascending=False).head(10)
```

The first needs nothing but the standard library, so Monty runs it. The second imports pandas: written as `python: monty` it would fail with "Monty can't run this cell: line 1: no module named pandas — Monty runs the standard library only; use a local python", so it names the project's `.venv`, and its result arrives as a DataFrame table.
