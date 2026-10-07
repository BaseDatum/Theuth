# 132 — as built, ENG (engine half of ATTENTION-ROWS-AND-ORDER)

Branch `feat/132-eng`, cut from `7b518912`. Contract: `docs/design/132-attention-rows-and-order.md` §2.1–2.3.

## Shape changes against §2 (read first)

- `ALREADY_WAITING` is a function, not a const: `tasks::ledger::already_waiting(decision: &str) -> String` (the id is interpolated; a `&str` const cannot carry it). Text as in the design.
- `Ledger::review_packet` now takes the `tasks::ReviewPacket` message (gains `options`, `recommended`, `if_wrong`) instead of six positional args. `Ledger::move_to` gains `next: Option<u32>` after `done_because`.
- `WakeReason::Answer` now applies to a `done` task (`WakeReason::applies_to`): the `**next**` wake of D3 would otherwise be dropped as stale by `wake()`/`dispatch()`/`drop_stale_wakes`.

## File map (path:line)

_(filled per step below)_

## What was verified and how

_(per step)_

## Deviations from the design

_(per step)_

## Open items

_(per step)_
