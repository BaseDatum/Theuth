# endless-eval

Offline comparison of single-unit vs batch (≤16 units/call) endless-memory summaries.
Replays `crates/agent/tests/fixtures/endless/B.main.jsonl` (already units: `kind`, `text`).
Python 3 stdlib only.

Prompts: system = `compact_prompt.txt` with `{agent}`→`theuth`; user = `Step::render` layout (earlier ≤24 lines, ruler, instruction).
Units whose `kind: text` ≤ `--node-bytes` are free (verbatim, no call) and excluded from calls/metrics (count printed).
Batches: consecutive non-free units, ≤`--batch-units` and ≤24000 text bytes; free units do not break a batch.
Not modelled: `<chat>` context block, retries/corrections after a guard trip, merge steps.

Guards mirrored (all of `guards::check`; word lists parsed from `guards.rs` at start): empty, format (G4), copy (G1: 6-gram of ruler/prompt phrases), grounding (G2), status (G3), size. Pass rate = passing lines / units; a failed call or missing batch number counts as empty.

Metrics: calls, pass rate, mean line bytes, key-term retention (identifiers, paths, numbers; units with no terms skipped), tokens from provider `usage`, USD from `--price-in/--price-out` (per M tokens, default 1/5).

## Usage
```
export ANTHROPIC_API_KEY=...   # or any var named by --key-env; unset => exit, no request
python3 eval.py --model MODEL [--provider anthropic|openai] [--base-url URL] [--key-env VAR]
                [--limit N] [--batch-units 16] [--node-bytes 512] [--out /tmp/137g-e/eval/result.json]
```
Defaults: anthropic → `https://api.anthropic.com` (`/v1/messages`); openai → `https://api.openai.com` (`/v1/chat/completions`).

## Against the bundled fake
```
python3 fake.py 18737 &            # prints PORT=18737
FAKE_KEY=x python3 eval.py --provider anthropic --base-url http://127.0.0.1:18737 \
  --model fake --key-env FAKE_KEY --limit 40
kill %1
```
Use `--provider openai` for the other endpoint. The fake echoes truncated input, so its numbers only prove the plumbing.
