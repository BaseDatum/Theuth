
## Meta studies — the loop improves the loop

`/autoresearch meta` (or "improve how autoresearch runs studies") drafts a study whose artefact is the **study mechanism**: four files in the plugin-owned git repo `<data_dir>/autoresearch/mechanism/` — `protocol.md` (the brief's protocol), `fire.md` (the fire prompt), `advisor.md` (the advisor's system prompt) and `mechanism.yaml` (brief caps, `plateau.window`, `novelty.threshold`, `defaults.crash_retries`, categories). The gate, scoring, custody, budgets, the toolset and this skill are not in it and cannot be put in it: `mechanism.yaml` refuses any other key, and theuth's own prompts are never the artefact.

Scoring is a **suite of fixture studies**: each outer experiment copies every training fixture into its own repo and runs one ordinary inner study per fixture and seed (8 experiments, $2, `parallel: 1`, the compiled gate) under the candidate mechanism. A fixture's score is its inner study's normalised holdout gain, (base − final) / (base − reference); the outer decision is the paired gate over fixture × seed. Held-out fixtures run only as the outer holdout, every second keep and at the end, and never reach the outer session.

Start from the preset and ask only three things:
- **Budget.** `budget.usd` defaults to $60; one outer experiment is about $13 (4 fixtures × 2 seeds × ≈ $1.20 plus the incumbent's re-run every third experiment), so $60 buys 4–5 outer experiments. `budget.experiments` defaults to 6.
- **Window.** An outer experiment takes 2–3 hours of wall time; offer the user's idle window (`window:`), or none.
- **Suite.** Training `toy-sort toy-dedupe toy-wordfreq toy-pareto`, held out `toy-route toy-json toy-lru`. The user may name fewer; the `suite` row of the draft panel lists them.

The draft (the fields are the 140 ones; the suite rides on them):

```yaml
slug: meta
goal: "Improve how theuth runs studies: the protocol, the fire prompt, the advisor's prompt and the brief's caps."
repo: /Users/me/.theuth/autoresearch/mechanism
axes: {scoring: suite, artefact: mechanism}
metric: {name: suite_gain, direction: higher, min_effect: "0.02"}
bench: {command: "toy-sort toy-dedupe toy-wordfreq toy-pareto", timeout: 4h, repeats: 2, exclusive: false}
holdout: {command: "toy-route toy-json toy-lru", every: 2, repeats: 1}
scope: [protocol.md, fire.md, advisor.md, mechanism.yaml]
models: {target: <a cheap executor for the inner studies>}
budget: {usd: 60, per_experiment_usd: 20, hours: 24, experiments: 6}
```

`bench.command` names the training fixtures, `holdout.command` the held-out ones, `bench.repeats` the suite runs of the incumbent at Start, `models.target` the inner executor, `bench.timeout` the time one suite run may take. The repo must exist: Settings › Autoresearch › Mechanism seeds it from the compiled defaults the first time it is opened (version 1); if `<data_dir>/autoresearch/mechanism` is missing, ask the user to open it once.

While it runs, `autoresearch_status` shows the suite's mean gain as the metric; the Study view shows the per-fixture bars, training and held-out apart, and the mechanism diff of each keep. Finalize ends with "mechanism v<n> ready to adopt": adoption is the user's press in Settings › Autoresearch (`autoresearch.mechanism {op: adopt, version}`). New studies pin the adopted version at Start; running studies keep theirs; Revert returns to the compiled defaults. Inner studies carry `created_by: meta:<slug>/<fixture>/<seed>` and are not the user's: do not report on them one by one.

Never: edit a fixture (they are evaluator files; custody refuses the keep), suggest lowering the gate or changing budgets as a mechanism change (there is no key for them), or run a meta study of a meta study (an outer study always runs the compiled mechanism).
