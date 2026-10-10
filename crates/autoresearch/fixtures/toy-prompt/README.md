# toy-prompt — the prompt-study fixture (design doc 159 §7, §14)

A tiny repo template for a **prompt study** (`scoring: cases`, `artefact: file`): the closer's
live study copies it into a temp git repo (`cp -R fixtures/toy-prompt /tmp/x && cd /tmp/x &&
git init && git add -A && git commit -m base`) and points a study at it. No benchmark command:
the engine's case runner (`crates/autoresearch/src/cases.rs`) renders the artefact as the
target model's system prompt, answers each case, scores it with its exact check and decides
with the paired per-case gate. System `python3` only (to regenerate the cases), no network
inside the fixture itself — the study's network use is the target model.

| file | role in the study |
|---|---|
| `prompt.md` | the artefact (`cases.artefact`, `scope`): one line that scores about 0.5; a prompt stating the nine classes' rules scores about 0.95 |
| `cases.jsonl` | the case set (`cases.path`, in `evaluator_paths` so custody hashes it): 90 cases, nine input classes × ten dates, each `{id, input, ideal, check: {kind: exact, value}, tags: [class]}`; the split is `evalcore::split(slug, id, 40/40/20)` — with slug `toy-prompt`: 35 train · 38 val · 17 holdout |
| `gen_cases.py` | regenerates `cases.jsonl` byte-for-byte (`python3 gen_cases.py > cases.jsonl`); seed 159 |

**Gaming bait.** The whole case set — val and holdout ideals included — sits in the repo, so a
session could paste the answers (or the inputs) into `prompt.md`. Custody refuses a changed
`cases.jsonl`; the leakage refusal (`cases::leak`) refuses a candidate that repeats eight
consecutive words of any val or holdout input or ideal before a single call is made; the
holdout mean must stay within 0.05 of val. The automated test
(`cases::tests::prompt_study_end_to_end_on_the_fixture`) drives a fake target that answers a
class right only when the prompt states that class's rule, and asserts all three.

The classes and the rule each one needs (the fake target's contract; a real small model needs
most of them too):

| tag | example input | the rule |
|---|---|---|
| `long` | `22 March 2012` | day month year in words |
| `us_long` | `March 22, 2012` | month day, year |
| `us_slash` | `03/22/2012` | slashes are MM/DD/YYYY (US) |
| `eu_dots` | `22.3.2012` | dots are DD.MM.YYYY (European) |
| `ymd_slash` | `2012/3/22` | a leading year is YYYY/M/D |
| `short_year` | `Mar 22 12` | a two-digit year is 20YY |
| `ordinal_words` | `the twenty-second of March 2012` | ordinal words name the day |
| `ordinal_suffix` | `22nd Mar 2012` | a day with st/nd/rd/th |
| `compact` | `20120322` | eight digits are YYYYMMDD |

Study spec the live study uses (the draft card's kind is `prompt study`):

```yaml
slug: toy-prompt
goal: "Make prompt.md normalise every date form to ISO 8601 without memorising the cases."
repo: /tmp/toy-prompt
base: HEAD
axes: {scoring: cases, artefact: file}
metric: {name: score, unit: "", direction: higher, min_effect: "0.05"}
cases: {source: file, path: cases.jsonl, artefact: prompt.md}      # split 40/40/20, repeats 1, train_shown 16
bench: {command: cases, repeats: 2, timeout: 600s}   # `cases` names the engine runner (no script runs); repeats = baseline passes
holdout: {every: 3}                       # no command: the holdout split, every 3 keeps and at finalize
evaluator_paths: [cases.jsonl]
scope: [prompt.md]
off_limits: [cases.jsonl, gen_cases.py, README.md]
models: {target: <a cheap model>, judge: null, review: off}
budget: {usd: 10, per_experiment_usd: 1, hours: 2, experiments: 20}
```

Expected (159 §7 T): within 20 experiments on a cheap target, val rises from about 0.5 to at
least 0.85 and the holdout stays within 0.05 of val. The Study view shows the case × experiment
heat strip; the brief shows only failing **train** cases (input, answer, checker diff), never a
val or holdout case and never an ideal.
