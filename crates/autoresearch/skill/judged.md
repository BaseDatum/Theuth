
## Judged studies (no scalar)

This section replaces the judge-only decline above (159 §8). When the quality the user wants is a judgment — prose, a report, UI copy, an answer style — and no number they trust exists, draft a **judged study** instead of declining. The loop does not score a number; it shows a judge two outputs for the same prompt, blind and in both orders, and keeps a change only when the judge prefers the candidate often enough that chance and position bias cannot explain it.

Set in the draft:
- `axes: {objectives: judged, scoring: pairwise}`; `metric: {name: win_share, direction: higher, min_effect: "0.1"}` — `min_effect` is the preference margin δ: a keep needs the win share's Wilson lower bound above 0.5 **and** p̂ ≥ 0.5 + δ (about 16 of 20 prompts at the first look, 29 of 40 at the second).
- `judge.rubric` — the yardstick in plain words, 3–8 criteria the user would apply themselves ("covers every listed feature, invents none, at most 60 words"). It is stored in the spec, so the study session can read it but never edit it. Write it with the user; it is the study's definition of better.
- `judge.prompts` — a repo-relative JSONL file of `{"id": …, "input": …}` prompts, 40 or more; the plugin snapshots it at Start and the session is never judged on anything else. Each experiment judges 20 prompts, then 20 more only when the first look is promising. `judge.look` changes the 20.
- `models.judge` — leave it null: the default is a model of **another vendor** than the executor (the reviewer's rule), which resists self-preference. Name one only when the user asks.
- The artefact. A **code artefact** (a generator, a template, a script) keeps `bench.command`, which must write one file per prompt to `$AUTORESEARCH_OUTPUT/<id>.txt` (it also gets `AUTORESEARCH_CASES`, the snapshot's path); the check (`checks.command`) runs it into a temp dir and verifies every file exists. A **prompt artefact** (`axes.artefact: file`) is the one plain file path in `scope`; the plugin runs it over the prompts on `models.target` and writes the answers itself.
- `secondary_guards: [{name: length, max_regression: 20%, direction: lower}]` — offer it by default. Output length is always reported per experiment; the guard refuses a keep whose outputs grew more than 20 % in words, which is how padding gets caught.
- Budgets: about 2 × 20–40 judge calls plus one generation per experiment, ≈ $0.70 at a $0.004 judge; pre-fill `$20 · 25 experiments · 4 h · $1.50 per experiment`.

What the user should know (say it when you draft):
- **The probe.** Start judges six base-against-base pairs in both orders. A judge that picks A in more than 70 % of identical pairs, or ties fewer than half, is refused with the numbers; pick another `models.judge` then.
- **The human check.** After every fifth keep (`judge.human_every`) the user sees one pair from that keep — two outputs, unlabelled, in random order — and answers A, B or same. The Study view shows judge–human agreement; fewer than 3 of the last 5 agreeing pauses the study with "the judge disagrees with you".
- **The ladder.** Kept versions are rated by Bradley–Terry over every judged pair (θ ± 2 se, Elo-style), so the user sees whether later keeps are still gaining or the judge is drifting.

Draft card line: "Improve `<artefact>`, judged by `<judge>` on 20–40 prompts in blind pairs, both orders; you see one pair every 5 keeps."

Setup rows this kind adds: **Prompt set** (`judge.prompts`), **Rubric** (`judge.rubric`), **Judge** (`models.judge`, `judge.look`, `judge.human_every`). Decline only when the user cannot write a rubric or a prompt set: without them there is nothing to judge against.
