# 137F-E notes — endless settling stalls; settle-model switch ignored

## Contract (phase A, unchanged in phase B)
- `Stalled` picks the frontier's failed node (`blocking: true`). If the frontier is not failing, it picks the failed node with the most consecutive failures (ties: lowest key) with `blocking: false`.
- `next_retry_at` is wall time now + the remaining backoff (`Instant`-based), RFC 3339 to the second, `Z`. It is `None` while that node's job runs.
- `error` is the raw error collapsed to one line, first 300 chars. `model` is the id of the compactor that ran the failing call.
- `SessionMemory.stalled` is `None` for an epochs session. `endless_retry` on an epochs or unattached session changes nothing and returns `session_memory`.

## Decisions (phase B)
- **E1 resolution: eager, not lazy.** Every link of the chain now pushes a change:
  - `Endless::set_config`;
  - `Endless::set_title_model` (from `SetTitleModel`);
  - `Endless::set_session_model` (from `SetModel` with a session, once with the final model and effort — R1-6 — and from `apply_effort`);
  - `Endless::set_catalog` (R1-16: from AddProvider, RefreshProviderModels and RemoveProvider, the three places the supervisor reloads the catalog);
  - `attach`, which switches again after its insert (R1-2);
  - `endless_retry`.

  Each push re-resolves the compactor of the affected states (`pump::reresolve`) and wakes their pumps, so a changed `jobs` applies at once (R1-7). `State.current` holds `(key, compactor, cancel token)`. Every call reads it at spawn time, so a call always uses the compactor in force. The cache key is `model id | effort`, so the compactor is rebuilt only when the key changes. The only session-model path is `Agents::set_model` (`SwitchModel` has no other sender).
- **Tests' factory.** `compactor_for_tests` is now `Fn(&resolved id) -> Arc<dyn Compactor>`. In tests the id is the raw chain id (no catalog).
- **E3 cancel.** Cancelling is per State: at a switch, a detach (R1-9) or an `endless_retry` (R1-10), the State's token is cancelled and a new one is installed. The `select!` sits in `call()` around the build and around the E9 permit wait, so `settle_call(Cancelled)` still runs. Under the `live` lock, `finish` and `JobGuard` drop the failure of any job whose token fired (R1-3), so a failure landing between `settle_call` and `finish` cannot re-insert a stale backoff. No `abort()`.
- **E4.** `finish` (and the E10 guard) now bumps `built_tx` on every outcome, so a waiting `settle` re-checks right after a failure. Before, it woke only when a node landed. `settle` returns `false` as soon as the frontier has ≥ 2 consecutive failures, **whether it is waiting out its delay or running its retry**. Deviation from the brief's "and is in backoff": the first live run (retry_secs 1, local_attempt_secs 30) showed a turn waiting 24.7 s for the third attempt of a node that had already failed twice. A success, `endless_retry` or a compactor switch clears the state, and then turns wait normally again.
- **E6 locality** (`compactor::is_local` / `local_endpoint`, narrowed in R1-1). A compactor counts as local when one of these holds:
  - provider or genai namespace is ollama / lmstudio / lm-studio / llama.cpp / llamacpp / llama-cpp / vllm / localai / koboldcpp / mlx;
  - the gateway base-URL host is a single-label name; or `*.localhost`, `*.local`, `*.internal`, `*.lan`, `*.home.arpa` or `*.ts.net` (R2-3); or an IP that is loopback / RFC 1918 / CGNAT 100.64/10 / link-local / unspecified / ULA (an IPv4-mapped IPv6 form included);
  - a catalog entry exists and has no `cost`.

  An id the catalog does not know (custom gateway names, Azure deployments) is cloud unless its provider or host says local. For genai's built-in adapters (no gateway) the base URL is not known synchronously, so the namespace decides. E9's queue key is the base URL, else the provider, else the namespace, else the id.
- **E6 bounds.** `build()` wraps each attempt (transport/auth included) in `timeout(attempt)`. `ATTEMPT_TIMEOUT` is gone. Stream options:
  - cloud (R1-12, R2-4): `stream_header_timeout` = `stream_read_timeout` = the attempt bound, so a silent reasoning model gets the whole attempt;
  - local: `stream_read_timeout` 120 s, no header timeout, because the attempt bound already covers time to first token. Ceiling: a slow box doing prompt eval for more than 120 s after headers but before the first chunk trips the read timeout. `local_ctx_bytes` keeps the prompt small enough that this should not happen.
- **E7 (R1-13).** Only a timeout (our attempt bound, or genai's stream header/read bound — matched on "timed out") after a candidate exists ends the loop on the shortest candidate (`tries` negative). A provider or transport error (429/5xx) fails the node as before. Only answered attempts are `endless_calls` rows.
- **E8.**
  - Budget: `local_ctx_bytes` for local. For a known window: `(0.9·window − reply cap) × 3 bytes/token − (COMPACT + step + (tries−1)·3 KB)`. The step (unit text) is never trimmed.
  - Trim: drop whole oldest lines after `<chat>\n`, keep the tail.
  - Ceiling: a trimmed ctx changes the cached prefix (cache miss), but it only happens when the view would not fit.
- **E8 reasoning (R1-14).** Only a direct Ollama endpoint gets `ReasoningEffort::Zero`, which genai's adapter sends as `think: false`; a gateway target does not. Assumption: Ollama accepts `think: false` for non-thinking models. I believe it only refuses `true`; not verified against a real Ollama. OpenAI-compatible local endpoints are left alone: genai would send `reasoning_effort: "none"`, which LM Studio / llama.cpp may reject.
- **E9.**
  - Queues: a `Queues` map in `Inner` holds `endpoint → (local_jobs, Gate)`. Changing `local_jobs` installs a fresh gate; calls holding permits on the old one finish there.
  - `Gate` (R1-8) is a small priority wait line, `(priority, arrival)` in a `BTreeSet` plus a `Notify`. It is cancel-safe: a dropped wait removes its ticket.
    - Priority: a level-0 job of a session that is not backfilling gets 1; merges and backfill get 0.
    - Aging (R2-2): every 4th grant goes to the oldest waiter whatever its priority, so merges and backfill cannot starve.
  - Jobs (R2-1): a local session may have one level-0 job and one merge waiting or running at once. A merge queued at the gate therefore never keeps the next frontier job from being spawned; the gate serialises the calls and the priority orders them. Cloud compactors keep the per-session `jobs`, with no engine-wide cap.
  - Order: the permit is taken before the `model_calls` row is reserved and before the attempt timer starts, so queue time is never counted or ledgered.
- **E10.**
  - R1-4: `call()` wraps the build in `AssertUnwindSafe(..).catch_unwind()` inside the select. A panic becomes `Err("compactor panicked")`, its `model_calls` row settles `failed`, and the node backs off.
  - `JobGuard` stays as the backstop. If the task dies outside the build, it releases `running` and records "compactor task ended unexpectedly", unless the job was cancelled.
  - Outer bound: attempt × tries + 60 s around the build.
  - R1-11: the pump's timer counts only failed nodes that are neither running nor due, so it no longer spins every 50 ms.
- **EndlessConfig bindings.** The 4 new keys are `ts(as = "Option<…>", optional)`, so existing UI literals still type-check. serde defaults the missing keys, and `Memory.tsx` spreads the whole object on save, so they survive a save.

## For theuth (not done in this wave)
- **Ollama `num_ctx`:** genai's Ollama adapter builds `options` only from temperature/top_p/max_tokens/seed/stop and ignores `extra_body`. There is no way to pass `num_ctx` without changing genai (rust-genai `src/adapter/adapters/ollama/adapter_impl.rs` ~L70-98). With the default `num_ctx`, Ollama silently truncates long prompts from the front. `local_ctx_bytes` (24 KB ≈ 8k tokens) mitigates this. A genai change (an `options` passthrough) would let the engine set `num_ctx = prompt + 600`, rounded up to 1024.
- **Reasoning on OpenAI-compatible local servers:** no portable "off" switch in genai; see E8 above.

## Live proof (scratch engine, final R1 binary)
Setup:
- `target/debug/t --headless --data-dir /tmp/137f-e/engine3`, started with `env -i HOME=/tmp/137f-e/home …`.
- RPC on 127.0.0.1:7963; mesh relay/discovery off; slack/signal/telegram/meeting/link off; `secrets.key_store: file`.
- Two stdlib fakes, `/tmp/137f-e/fake_oai.py`: `slowlocal` (:7961, never answers a POST) and `fastcloud` (:7962, streams an echo of the step).
- Both registered with `add_provider` (fake keys) as `slow/m` and `fast/m`.
- Compactor `slow/m` (`endless_settings_set`: `retry_secs 1`, `local_attempt_secs 30`); session model `fast/m`.
- Driver: `/tmp/137f-e/proof.py`; full log: `/tmp/137f-e/proof.txt`; fake request logs `/tmp/137f-e/{slowlocal,fastcloud}.log`. No screenshots (engine only).

Both fakes are on 127.0.0.1, so by the host rule both count as local. They have different base URLs, so they use separate E9 queues. The switch does not depend on locality.

```
23:31:17 create_session{memory: endless, model: fast/m}; turn 1 (2256 B) 0.5s; slowlocal POST 23:31:17
23:31:52 stalled {"failures": 1, "error": "timed out after 30s", "next_retry_at": null, "model": "slow/m", "blocking": true}   (retry in flight since 23:31:48)
23:31:52 endless_retry -> stalled null; slowlocal POST 23:31:52 (the 23:31:48 call cancelled, R1-10)
23:32:25 stalled failures 1 · 23:32:56 stalled failures 2 (blocking: true)
23:32:56 turn 2 took 0.5s (settle_secs 45); engine log "endless settle: frontier failing; not waiting"
23:32:57 endless_settings_set model=fast/m (no restart); log "endless: compactor switched … from=slow…"
after 0.3s session_memory {"settling": false, "unbuilt": 0, "units": 4, "model": "fast/m", "stalled": null, lines 0..3 all built}
model_calls kind=endless: slow/m failed 3 · slow/m cancelled 2 (retry + switch) · fast/m settled 1; endless_nodes (0,0) model fast/m tries 1
```
Earlier runs:
- Run 1 (`/tmp/137f-e/proof-run1.txt`), before the E4 change: turn 2 took 24.7 s, because it waited for the running third attempt.
- Run 2 (`/tmp/137f-e/proof-run2.txt`), phase B binary: same results as run 3, without the retry step.
