# 119 — Attention: a dismissal hides only the version that was dismissed

Branch `fix/119-attention-dismiss-versioned`, base v2.9.2 (`11ce6f28`). Number 119 was free (no `docs/design/119-*`, no other branch / worktree).

## Bug

`State::hidden` (`crates/roster/src/attention.rs:190-192` at base) treated any `dismissed_ms` as "hidden for good", while the review / blocked item ids are fixed per task (`review:<task>` via `tasks::packet::review_key`, `blocked:<task>`) and nothing ever clears a row in `attention_state`. Sky dismissed ~14 review items at 16:05 so the desks could re-submit them as mini-board packets; the fresh `review:<task>` packets landed at 18:34 and stayed hidden behind the old dismissals (14 of 17; the 3 visible ones had no item to dismiss at 16:05).

## Rule

A dismissal applies to the version of the item the user saw, not to the id forever:

```
hidden = dismissed_ms.is_some_and(|d| d >= item.at_ms)
      || snoozed_until_ms.is_some_and(|u| u > now_ms)
```

applied in `derive`'s `retain` with each item's `at_ms`. A later version (newer `at_ms`) surfaces with no action by anyone. Snooze stays time-based.

## `at_ms` per item

- `review:<task>`: `max(t.updated_ms, packet.at_ms)` when a review packet exists, else `t.updated_ms`. A re-submitted packet is a new version even when the task row did not change.
- `blocked:<task>`: no packet is looked up there; `t.updated_ms` stays. A task edit after the dismissal surfaces it.
- `decision:<id>`, `ask:<id>`, `show:<call_id>`, hand-offs, secrets: unique ids, never reused; `at_ms` does not move on its own, so a dismissal keeps hiding them exactly as before. `shows()` still filters `dismissed_ms IS NULL` in SQL (`attention.rs:772`) — unchanged, correct for unique ids.

## Thread notes

`Ledger::say` (`crates/tasks/src/ledger.rs:279`) only appends to the thread doc and `note_disposition` only writes an event; neither bumps `updated_ms`. A dismissed review therefore does **not** resurface on a thread note. It does resurface on a field edit (`update`, state transitions, checkout / release, reassign) — accepted: something changed, look again.

## Tests (`attention.rs` `mod tests`)

- `dismissal_hides_only_the_dismissed_version`: review dismissed at 60, packet at 70 → visible; dismissed at 70 / 80 → hidden; snoozed until 200 at now 100 → hidden, until 90 → visible; no packet, `updated_ms` 40, dismissed 50 → hidden. Also asserts the packet time wins over `updated_ms`.
- `blocked_item_resurfaces_on_update_and_decision_stays_dismissed`: blocked + decision items dismissed at 50 with `at_ms` 40 → both hidden; task `updated_ms` → 60 → the blocked item is back, the `decision:` item stays hidden.
- Pre-existing tests dismissed at `1` items whose `at_ms` is later — `hire:amy` (created 20) in `derives_every_kind_and_hides_dismissed_or_snoozed`, `handoff:h1` (50), `show:c1` (50); those values were incidental and only held under "Some = hidden for good". Moved to the item's `at_ms` (= dismissed the version shown).

## Gates

```
export PATH=/Users/Sky.Moore/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:/Users/Sky.Moore/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH
export CARGO_TARGET_DIR=/Users/Sky.Moore/Develop/theuth-119/target
cargo test -p roster
cargo clippy -p roster --all-targets -- -D warnings
```

`crates/tasks` untouched (no `cargo check -p tasks` needed). `cargo fmt -p roster` on `attention.rs` only.

## Operational note

No schema change, no RPC / UI / bindings change (`State` is engine-internal, `AttentionItem` unchanged). Existing dismissals from before this build are compared under the new rule — items re-submitted after a dismissal surface after the engine restarts on this build; no DB surgery needed.

Follow-up (not done here): the desktop column receives the list per `attention_list` call; if the UI caches it and only refreshes on its own events, a resurfaced item appears on the next list fetch rather than instantly. Not verified in `crates/desktop/ui`.
