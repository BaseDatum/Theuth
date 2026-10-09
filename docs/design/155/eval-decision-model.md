# Decision model as a terminal-state classifier — evaluation (2026-10-09)

Evaluation of the local decision model (decide / decide_many, checkpoint gliner = GLiNER2.5-Decide, zero-shot, no capture) for naming a terminal coding agent's state from its screen, run 2026-10-09 16:57–17:45 at Sky's request while discussing herdr-style PTY monitoring. SUPERSEDES the optimistic 48-sample pilot (37/48 screen-only) of the same afternoon — the pilot screens were written by the orchestrator and were too easy.

## Data (~/.theuth/briefs/2026-10-09/herdr/state-eval/)
- real.jsonl: 224 usable REAL screens (working 83, idle 111, error 30, blocked 0; shell 108, omp 90, other 26) from 31 sessions / 110 jobs, labelled by one theuth subagent (single pass, `why` per row; real-notes.md). Sources: sessions.sqlite inner_calls screen/send/wait/start outputs, <job_idle> messages, pty_screens. 67 % of inner_calls outputs are stored cut at ~2,000 chars (bottom of the screen lost) and were dropped. ZERO real approval prompts exist: omp always ran with full permissions.
- synth.jsonl: 260 SYNTHETIC screens by another subagent, 65 per label, 10 programs × 26, 37 % hard cases, each with a plain-words signals line.
- Loading trap: read_file truncates lines over ~2,000 chars, so JSONL with long screens must be converted first (conv.py → *.rec, one screen line per file line).

## Results
- 4-way choice (working / waiting for a person to approve or answer / finished and ready for the next instruction / stopped because of an error): REAL 42 % full screen, 47 % last 10 lines, 48 % bottom-first; 25–32 false "waiting" verdicts on 224 real screens that contain none. SYNTH 49 % (blocked recall 12–15/65, idle 12–14/65, working over-predicted at 39 % precision, error 47–49/65). Signals + screen 47 % (blocked 7/65). Full screens were truncated at the 512-subword cap in 114/224 real rows — feed the last ~10 lines.
- Busy vs quiet from the screen (bool "The program is still busy right now: a spinner, a progress bar or streaming output shows it is working."): REAL 120/224, SYNTH 131/260 — a coin flip, 85/126 false busy.
- Signals only (synthetic): says working 53 times, right 52; abstains on every quiet state — signals are the busy/quiet gate.
- Error (3-way over quiet screens, last 10 lines): REAL recall 26/30, precision 26/32; SYNTH recall 51/65, precision 51/62.
- Strict prompt bool S1 "The screen shows a specific question, permission prompt or list of options the user must answer before the program can continue.": false alarms 1/224 real, 3/195 synthetic non-prompts (all on error screens); recall on synthetic prompts 25/65 (38 %).
- "Finished, empty prompt" bool S2: idle recall 16/111 real, 10/65 synthetic — useless.
- Composite on synthetic quiet screens (error from 3-way, else S1 vs S2 p(true)): 136/195 (70 %), blocked 25/25 precise but 25/65 caught.
- Speed on the loaded machine: 144–586 ms per row batched (longer states cost more), ~430 ms for a single decide().

## Conclusion
Zero-shot, the model must not decide the state. Use it only in its strong zone: an error check and the strict "must answer" check as an optional reason on an alert that the deterministic signals already raised ("went quiet" = your turn). Anything more needs a trained family (capture + labels) evaluated on held-out real screens, and real prompt screens to evaluate against.


Data: ~/.theuth/briefs/2026-10-09/herdr/state-eval/ (real.jsonl 227 rows, synth.jsonl 260 rows, real-notes.md).
