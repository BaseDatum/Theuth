# Double — the model of your own decisions

Double is a small local model of *you*: it watches every decision card theuth puts in front of you — tool approvals, `ask_user` questions, attention items, decision packets, draft choices — predicts what you will answer before you see the card, and learns from what you then do. Over time, class by class, it earns the right to pre-select the answer and, where you allow it, to answer for you after a short grace. It runs on your machine, on the decision model theuth already ships; nothing leaves your mesh. Design doc 126.

Off by default: `double.enabled: true` in `config.yaml`, then restart the engine.

## What it needs

- **The journal.** Double learns from the `questions` table of the session store (migration 0033): the card, your answer and how long you took. An engine older than that writes no rows, so double starts cold the day the 0033 engine runs; there is no backfill from older history (Revision 1 ruling 8).
- **Approvals on.** With `tools.approvals: false` nothing ever asks you, so there are no approval cards to learn from or answer; double then learns from `ask_user` questions only. Turn approvals back on (Settings › Approvals, `/approvals on always`) if you want it on tool calls.

## What it learns from

- Your answer to each card (Allow / Deny, the option you picked), the time you took, and whether you clicked the suggestion or overrode it.
- Hindsight that arrives for free in the same turn: a correction you type after an approval, an undo, a tool error right after an allow. These are weighted lower than a direct answer.
- Your own cards only (Revision 1 ruling 7). A subagent's, desk's or routine's approvals are learned under their own class (`…:subagent`, `…:desk:<agent>`, `…:routine:<name>`), never mixed with yours.

## What it never does

- It never sees **secret requests, desktop handoffs or MFA** cards, and never answers them.
- It never answers a **destructive** class above L1: it can suggest, it cannot act.
- It never answers a **mutating** class at L2 unless you opt that class in (Settings › Double › class, or `double_cap(class, 2, mutating_opt_in=true)`).
- It never rewrites the model's question or its `recommended` option; the journal stays the record of what was asked.
- It never answers twice: after a restart an already-answered card is honoured from the journal, not asked again.
- Cards from a session marked "don't learn from this one" are kept as excluded rows (weight 0) so the counts stay honest, and are never trained on.

## The ladder, in plain words

Every card lands in a class: `kind:shape:requester`, e.g. `approval:shell:mutating:git push:root` — the kind of card, a normalised shape of what it asks (the first two tokens of a shell command, the first directory of a path, the tool and its argument keys, a hash of an ask's option labels), and who asked. Each class climbs its own ladder; nothing transfers between classes.

| rung | what you see | how it gets there |
|---|---|---|
| **L0 observe** | a muted chip `double · Allow · 71 %`; nothing pre-selected | every class starts here |
| **L1 suggest** | the predicted answer pre-selected; one tap confirms | ≥ 20 labelled cards in 30 days, accuracy ≥ 90 %, well calibrated |
| **L2 act with grace** | `double answers Allow in 30 s — tap to take over`; when the 30 s run out it answers for you | ≥ 50 cards, accuracy ≥ 97 %, no override in the last 20, and the class's cap allows it |
| **L3 silent** | answers at once; one line in the daily digest | not in this version: caps default to 2 |

Falling is immediate: any override of an L2 answer drops the class one rung; three wrong suggestions in the last 20 at L1 drop it to L0. At L2 the grace **is** the undo for a mutating approval — nothing has run until the countdown ends, and answering the card yourself cancels it (that counts as an override only if you answer differently).

`none` on the chip means the double is unsure: the card behaves as L0 whatever the class's rung.

## Where the controls are

| control | desktop | TUI | chat | config |
|---|---|---|---|---|
| on / off | Settings › Double | `/double off` (restart) | — | `double.enabled` |
| status and numbers | Settings › Double | `/double status` | `double_status()`, `double_classes()`, `double_class(key)` | — |
| pause learning (predictions continue, labels stop) | Settings › Double | `/double pause [off]` | `double_pause(on)` | — |
| per-class cap and mutating opt-in | class row in Settings | `/double cap <class> <rung> [opt-in]` | `double_cap(class, rung, mutating_opt_in)` | `double.ladder.caps`, `double.ladder.mutating_opt_in` |
| train now | Settings › Double | `/double train [a\|b] [venue]` | `double_train(tier, venue)` | `double.training.*` |
| roll back to a version | Settings › Double | `/double rollback <version>` | `double_rollback(version)` | — |
| forget | class row · example popover · Delete all | — | `double_forget(class \| example_ids)` | — |
| take over an L2 card | tap the card | answer the card | — | — |
| don't learn from this one | chip popover toggle | — | — | `double.capture.<kind>: false` per kind |
| which cards it learns from | Settings › Double | — | — | `double.capture` |
| grace length | Settings › Double | — | — | `double.ladder.grace_secs` (30) |

Every key except `enabled` and `model.*` applies live and is written back to `config.yaml`.

The reasoning model gets the same view through the `double_*` functions and the `double` skill; it treats a double answer as your answer and never argues with it. Its mutating functions (`double_pause`, `double_train`, `double_forget`, `double_rollback`, `double_cap`) are for when you ask.

## Training, privacy and the mesh

Three tiers. **Tier A** (temperatures, a nearest-neighbour memory, small per-class heads) runs in-process, in seconds, after every 25 labels or nightly, deterministically. **Tier B** fine-tunes a LoRA adapter on the encoder when 200 new labels or 7 days have accrued, inside `double.training.window` (default 02:00–06:00), only on AC power, with you idle for `idle_min`, no meeting running and nominal thermals, capped at `run_cap_min` per night. **Tier C** (full fine-tune) is an owner action from Settings, never automatic. A new version only becomes active if it beats the active one on held-out rows and a golden set; otherwise it is rejected and the run says why.

Where Tier B runs is `double.training.venue`: `auto` picks a paired mesh engine with a GPU that is not a laptop and **never a sandbox VM**; `local` is this machine; `node:<name>` a specific engine; `off` disables training. Bundles shipped to a node are scrubbed (secrets redacted, card text capped) and go only to your own paired engines with `share_data: true`; nothing is uploaded to theuth or anyone else, and the separate 57 §7.1 `share` consent stays off for the `double.*` families. The store is `~/.theuth/double/double.sqlite`, engine-local, not replicated; rows older than `retention_days` (365) are dropped except golden rows and the last 1,000 per class. **Export** (Settings › Double) writes a scrubbed JSONL + embeddings archive under `~/.theuth/double/export/`; **Delete all** removes the store, the models and the runs and leaves the config.

## FAQ

**Why did it answer X?** Tap the chip (desktop) or open the class (`double_class(key)`): the class key and rung, the nearest past decisions with the date and what you chose, the head's contributions, and the version that predicted it. Every answer is recorded with `answered_by: double` in the journal and names its version, so a rollback shows up as a vertical line on the class's curve.

**Why did it stop acting?** You (or hindsight) overrode it: one override at L2 drops the class to L1 immediately; it has to re-earn L2 with 50 clean cards. The class row shows the strike. A cap can also hold it: "L1 (cap: mutating — opt in to allow L2)".

**How do I make it forget?** Per class: the class row's *Forget* or `double_forget(class)` — every example, label and prediction of that class goes and the class is back at L0. Per example: the popover's *Forget this* or `double_forget(example_ids)`. Everything: Settings › Double › *Delete all*. Forgetting is immediate and irreversible; the next Tier A run no longer sees the rows, and the next Tier B run trains without them.

**Why is there no chip on this card?** The class is new and the budget (150 ms) was missed, the card kind is off in `double.capture`, the card is a secret/handoff/MFA request, or double is disabled or not yet started.

**Does it slow cards down?** No. The card renders first; the chip arrives after it and never delays it.
