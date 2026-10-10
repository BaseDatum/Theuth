
## World studies

A *world study* (159 §11) optimises a metric that lives outside the machine — a landing page's conversion, an email template's reply rate, an endpoint's live latency — read from production, not printed by a benchmark. Set `axes: {scoring: world}` and a `world:` block. Set `bench.command` to the same measure command (Start still requires one; a world study never runs it as a bench — custody takes it — and `bench.repeats` is how many times the source is read for the baseline).

**When.** The user owns the deploy (a command of theirs puts a tree live for a slice of traffic), can read the metric per arm (a command or one of their read-only analytics tools), and has a rollback that is safe to run twice. **Never**: anything that spends money, anything without a rollback, anything where a bad arm hurts people. Say so and decline.

**The block.** `world: {deploy, rollback, measure | measure_tool, exposure: {max_fraction}, window, min_units, min_windows, max_windows, alpha, alpha_harm, tau}`.
- `deploy` and `rollback` are custody commands the plugin runs after the user's press — never the study session. They get `AUTORESEARCH_ARM` (`control` | `candidate`), `AUTORESEARCH_TREE` (the arm's sha), `AUTORESEARCH_ARM_WORKTREE` (that sha checked out), `AUTORESEARCH_EXPOSURE` (the arm's share) and `AUTORESEARCH_N`. Run `rollback` once yourself during setup to prove it is idempotent; never run `deploy` in setup.
- `measure` prints one line per arm, `CASE <arm> units=<n> mean=<x> var=<v> [<name>=<v> …]` (extra names are guard metrics; `CASE total units=<n>` is production's whole traffic, used for the exposure check). Run it once in setup and show the user the parsed arms. Or `measure_tool: {name, args, arms, arm, units, mean, var}` names a read-only tool from the user's registry (also listed in `calls`) and JSON pointers to the same fields.
- Defaults: `exposure.max_fraction` 0.1 (both arms together; each gets half; the rest of production is untouched), `window` 24h, `min_units` 500 per arm per window, `min_windows` 2, `max_windows` 14, `alpha` 0.01, `alpha_harm` 0.05, `tau` = `metric.min_effect` of the base mean.
- Put the deploy, measure and rollback scripts and everything they read in `evaluator_paths` and `off_limits`.

**How it runs.** Each experiment is one launch: the session edits the artefact, checks run locally, and `experiment_run` answers `pending` — nothing is live yet. The study pauses and the user gets a launch card (Approve / Refuse) in this chat. Approve puts the incumbent and the candidate live side by side at `max_fraction / 2` each; every `window` the plugin reads both arms and runs an always-valid sequential test (mSPRT) with a sample-ratio check, the guards and a harm test; a keep needs `min_windows` windows and `min_units` per arm; `max_windows` without a decision is inconclusive. Both arms are rolled back on every close. A keep moves the study incumbent only; production changes when the user lands it.

**Draft.** Pre-fill `budget: {hours: 336, experiments: 8}`; money is model spend only (about $0.30 per launch) — a world study spends nothing in the world (D4). The card's first line reads "Test `<scope>` on 10 % of traffic in two arms; every launch needs your press; 24h windows; rollback `…`".

**In the study session** (the brief says it too): after a `pending` verdict, end the turn with one line; do not call `experiment_log` — the harness closes the experiment when the windows end.
