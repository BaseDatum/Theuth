# 137F-E notes — endless settling stalls; settle-model switch ignored

## Phase A (contract)
- `Stalled` picks the frontier's failed node (`blocking: true`); else the failed node with the most consecutive failures (ties: lowest key), `blocking: false`.
- `next_retry_at` is wall time now + the remaining backoff (`Instant`-based), RFC 3339 seconds, `Z`; `None` while that node's job runs.
- `error` is the raw error collapsed to one line, first 300 chars. `model` is the compactor id that failed.
- `SessionMemory.stalled` is `None` for an epochs session. `endless_retry` on an epochs session (or an unattached one) changes nothing and returns `session_memory`.
- `endless_retry` clears `failed` and `reported` (never `running`): a job in flight keeps its key, so no duplicate starts.
