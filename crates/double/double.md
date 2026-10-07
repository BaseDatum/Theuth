---
name: double
description: How to read and respect the owner's double — the chip on approval and ask cards, what `none` means, the L0–L3 ladder with its 30 s grace, why a double answer is the owner's answer, and the double_* functions for its status, classes, pausing, training, forgetting, rollback and caps.
---

# double: the owner's stand-in

The double is a local model of the owner's own past decisions. Before the owner sees an
approval or ask card it predicts their answer and shows a chip: `double · Allow · 91 %`.
It learns only from what the owner then does. It is not you and it is not the owner; it is
the owner's habit, measured.

## Reading the chip

- `double · <choice> · <p> %` with ○ (L0) or ◐ (L1): a prediction, nothing has happened. At L1
  the choice is pre-selected; the owner still answers.
- ● (L2) with "answers <choice> in N s": the double will answer when the 30 s grace runs out
  unless the owner takes over. Keep working; the card resolves itself.
- `none`: the double is unsure. The owner decides; say nothing about it.
- No chip: the class is new, the card kind is not captured, or double is off.

## The rules

1. A card answered by the double (`answered_by: double`) is the owner's answer. Act on it
   exactly as on a human answer. Never re-ask, never second-guess it, never argue with it.
2. Never argue with a chip either. If you disagree with an L1 suggestion, say so in one line
   ("double expects Allow; I would deny because …") and let the owner decide.
3. Never try to shape a card to land in a class the double answers (a shell command that starts
   with a routine prefix and continues with something else). The class key includes the risk and
   the requester; destructive never passes L1; mutating needs the owner's per-class opt-in.
4. The double never sees secret requests, desktop handoffs or MFA, and never answers them.

## The functions (only when the owner asks about the double)

- `double_status()` — enabled, paused, version, venue, last run, accuracy vs baseline.
- `double_classes()` / `double_class(key)` — the rungs and the learning curve; a class key reads
  `kind:shape:requester`, e.g. `approval:shell:mutating:git push:root`.
- `double_pause(on)`, `double_train(tier, venue)`, `double_rollback(version)`,
  `double_cap(class, rung, mutating_opt_in)` — change it; `double_forget(class | example_ids)` is
  irreversible. All mutating: say what you are about to do and do it only on the owner's word.
