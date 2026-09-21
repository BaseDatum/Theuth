# 51 — Session lifecycle, project metadata, model cost, and terminal observation

Status: **as built · 2026-09-19 · implemented on branch session-lifecycle; automated checks recorded below**.

Extends [00](00-architecture.md), [07](07-context.md),
[12](12-course-correction.md), [14](14-jobs.md), [19](19-subagent-board.md),
[22](22-pty.md), [25](25-desktop.md), [31](31-link.md),
[35](35-artifacts.md), [36](36-survival.md), [37](37-secrets.md),
[47](47-todos.md), [49](49-search.md), and [50](50-agent-communication.md).
Source observations below were the 2026-09-18 starting points; the design body
is left as written. Where delivery diverged, the Status section lists the
deviation. The 00 amendment is accepted with the evidence recorded there.

## 0. Decisions

1. A private root has **one owning view**, an in-memory store, and a private
   scratch directory. Explicit leave closes its entire tree immediately;
   connection loss allows that owner 30 seconds to reconnect.
2. Private means no durable theuth session/history/memory records, not a sandbox
   or forensic erasure. Scratch files are real files. Native descendants inherit
   retention immutably. ACP and remote children are refused in private v1.
3. `meta.project` is one nullable root metadata value, backed by a dedicated
   `project TEXT` column. No generic metadata map/update framework, project
   object, workspace, repository inference, or registry.
4. Reserve each native request attempt before transport. Returned cost remains
   the primary observation; missing stays unavailable. Keep any estimate and
   provenance separate, never substitute it into or combine it with returned
   totals. Currency is explicit. Child cost survives until root deletion.
5. Consume standard ACP cumulative `UsageUpdate.cost`. An opaque remote total
   includes its hidden descendants once; it is not a per-turn cost.
6. `Jobs` owns the only terminal emulator. Read-only viewers ask for bounded,
   redacted, styled snapshots with 512 lines of normal-buffer scrollback.
   They never replay raw ANSI or become a second terminal state authority.
7. Existing actors own state and teardown. `rpc::Api` is the single command
   surface for TUI, desktop, mobile, and link. No new crate, resource registry,
   billing service, broker, or separate frontend protocol.

## 1. Philosophy and scope

Choose storage before the first input. Deleting a durable row afterward cannot
undo SQLite WAL, backups, indices, or attachments. Private trees must never
enter durable theuth session, artifact, communication, cost, search, knowledge,
or attachment stores. Their session records use memory; their owned file
outputs use a temporary root. This is a retention boundary, not an assurance
against swap, crash dumps, filesystem snapshots, provider retention, or escaped
processes. A tool's explicit filesystem/network/message effects outside that
boundary are not undone. Private mode displays this warning before first use.

Metadata is for grouping. Projects do not select cwd, models, permissions,
configuration, git remotes, or environments. Artifacts obtain project from their
session rather than copying it into artifact records.

Cost is observation, not an invoice. Missing usage is unknown, not free.
Context heuristics must not become billed tokens. Frontends re-read owner
summaries rather than summing event deltas. Bounded terminal observation follows
the existing browser-live ask/poll pattern, not a streaming subsystem.

Out of scope: private exports/conversion, remote-private execution, project
configuration/search syntax, historical cost reconstruction, currency conversion,
account billing dashboards, PTY resize, raw terminal replay, and unbounded
scrollback. Bounded retained scrollback is explicitly in scope. (PTY input
was out of scope here; 55 adds it as `pty_input` on the same viewers.)

## 2. Current source and proposed changes

The source map in the appendix identifies implementation seams.

- `Agents` and `Store` create sessions in the durable SQLite database today.
  Ordinary durable deletion refuses active work. Private close must not inherit
  that rule: it revokes admission, cancels the running turn and every tree-owned
  pipe/PTY job, then waits for process reaping during asynchronous cleanup.
  Session storage has no project column today.
- `SessionSummary` has no metadata container today. Add only the effective
  project field described below; do not introduce a generic metadata envelope.
- Artifacts use SQLite/blob storage; attachments are uploaded before a session
  through `put_attachment(bytes, ext)`. Spill, job, browser, and computer paths
  currently use the durable data directory. Private writers need routing before
  writing, not a later move or deletion.
- Knowledge harvest can write durable concepts automatically. Private mode must
  disable it and refuse harness-owned knowledge mutations.
- Native backend events are `genai::chat::ChatStreamEvent`; the current theuth seam
  does not preserve monetary cost. Catalog rates exist but are not a v1 input;
  context fallback counts are heuristic. ACP currently reads `UsageUpdate.used`,
  not its optional standard `cost`.
- Title generation is detached in `loop_.rs`; some diagnostics include title or
  reply content. It needs tracked lifetime, call accounting, and write fencing.
- Jobs already has a `vt100` parser, a bounded reader channel, and coalesced
  `JobScreen` events. The current parser uses 120×40 and zero scrollback;
  `PtyHandle::screen` returns redacted plain rows. Styled snapshots and bounded
  scrollback are proposed extensions, not existing fidelity guarantees.
- RPC event broadcasts are lossy. Current `new_session() -> SessionId` and
  `put_attachment(bytes, ext) -> PathBuf` stay unchanged. Additive wrappers below
  avoid ambiguous response shapes or silently breaking older clients.
- Desktop persistence today is narrower than the proposed private audit:
  `App.tsx:291` and `link/harnesses.ts` store link harness identities in
  IndexedDB; `localStorage` stores theme only. The inspected client does not
  currently persist transcripts, drafts, file caches, or previews. Do not
  describe those as existing caches; the rules below prevent adding private
  data to such persistence later.

## 3. Project metadata

Expose `meta.project: Option<String>` in session metadata; store one nullable
`sessions.project` value on the root only. Normalize once at the API boundary:
reject Unicode control characters (including NUL, tab, and newline), trim
leading/trailing Unicode whitespace, then require 1–128 UTF-8 bytes. Empty
input is invalid; `null` clears the value. Accepted normalized values retain
case and interior spaces; no slug, path expansion, or cwd inference.

This is a dedicated field and setter, not the first entry in an arbitrary
metadata map, JSON column, or generic metadata update API.

`set_session_project(session, project)` accepts any root or child, authorizes that
session, resolves its root, and mutates the root value; child calls are not
rejected and children have no independently mutable project. Commit before
publishing `SessionMetadataChanged`; every child summary exposes the effective
value. A fork copies it into its new root once, then diverges. Schedules, channel
and legacy lazy creation default to ungrouped unless an authorized creator
explicitly supplies metadata.

Session views group roots by project, first-seen order from the existing
pinned/newest list, with Ungrouped last. Children stay under their roots.
Artifact views group effective project → session → name/version. Join through
authorized session metadata; never copy project into artifact rows. A genuinely
missing durable owner is shown as Unavailable session. An unauthorized private
owner and its artifacts are omitted entirely, not represented by placeholders.
Labels are escaped display data. TUI uses headings; shared React uses collapsible
sections. No project routes, tables, actor, watcher, or search category.

## 4. Private ownership and access

### 4.1 Creation and one owner

`create_session({ retention, project, view })` selects storage before creating
rows. Durable is the default. Private requires an explicit owning view and an
RPC peer (or equivalent in-process TUI peer). The result contains a root/session
id and a random, unguessable reconnect lease generated by theuth.

There is exactly one owner record per private root: root-local view identity,
the existing stable `RpcPeer` identity, lifecycle generation, and a
cryptographically random lease secret. Reuse `RpcPeer` and its existing closed
signal; do not add a parallel lease actor. The secret is kept only in harness
and owning-view memory, never in URLs, logs, browser localStorage, database,
transcript, or model tools. A `ViewKey` is correlation, not authority.
Duplicate creation for the same request may return its existing owner result;
another view cannot attach or keep it alive.

Private retention is immutable. Native children inherit store, resource root,
project resolution, and lifecycle token. Private forks create a separate
private root with a new owner and in-memory conversation copy. No conversion to
durable is offered. ACP/node/A2A/remote child creation is refused **before
external dispatch** in private v1, even if the user accepts a retention warning.
Private trees cannot be scheduled, exported via A2A, or created by unattended
channel inputs. Provider model calls remain permitted with the retention warning.
Adapters request no-store/zero-retention handling where a provider offers a
verified request option, but provider-side retention is outside harness control;
private mode never guarantees that the provider does not retain the request.

### 4.2 Admission and disclosure

A private id alone grants nothing. Existing console authentication alone also
does not grant another view access. Every access path resolves the authoritative
session retention/generation and private owner peer, failing closed if unknown.
The same rule covers load/list/history/cost/todos, RPC subscriptions and replay,
artifacts/app queries and mutations, file/download/attachment routes, tools,
search candidates/snippets, notifications, approvals, and terminal snapshots.
Filter before shared subscription/broadcast delivery: session-list changes,
titles, project metadata, cost invalidations, `SessionClosing`, `SessionDeleted`,
and even existence-only events for a private tree go only to its captured owner
route. Non-owner clients receive neither rows nor invalidations from which they
could infer it. Private sharing is not implemented in v1.

Native child tools act under inherited tree authority. Cross-root peers cannot
discover, read, send intake to, or retain a private tree. Local same-tree native
communication uses its in-memory store. Deliberate outbound messages/files are
external effects and may survive at their destination; no automatic private
transcript projection or durable peer route is created. Global search cannot
merge private candidates before caller-specific filtering.

Private frontend state is proposed memory-only; this is not a claim that a
persistent transcript cache exists today. Never write private restoration/session
ids, reconnect secrets, drafts, downloaded-file caches, artifact/attachment
previews, rendered content, or push-notification payloads to IndexedDB,
`localStorage`, Cache API/service-worker storage, or another persistent client
store. Existing persisted link harness identities and theme preferences contain
no private session id or content. Clear in-memory transcripts, query/Zustand
state, drafts, files, previews, and rendered artifacts on leave, close, or
harness switch. HTTP private responses use `Cache-Control: no-store`. Backend
invalidation remains authoritative if a client crashes.

### 4.3 Leave and reconnect

Root→child→parent navigation, transcript pages, in-session artifacts, PTY
viewers, and overlays are ephemeral subviews inside the same root-local view
scope; opening or closing them does not release ownership. This may look like a
workspace in the UI but creates no workspace object. Leaving for another root,
new-session view, global explorer, or closing the owning view releases the
owner. Opening a new window is not another lease. The client sends
`leave_session` and discards local private state; a successfully admitted
explicit leave revokes ownership immediately.

The existing bound `RpcPeer` closed signal starts a fixed 30-second reconnect
deadline measured by the harness. The same root-local view may present its
secret on an authenticated new peer before the deadline; atomically rebind the
owner, invalidate the previous peer binding, and rotate the secret. View id or
authenticated user identity alone is insufficient. Activity by another peer
does not extend the deadline. A stale leave/disconnect from the previous peer
cannot close the rebound owner. While disconnected no other peer may read it.
Explicit leave, deadline expiry, or explicit delete transitions to Closing,
which never reopens. Harness restart loses the tree and lease; reconnect cannot
reconstruct it from scratch files. Until an explicit close is acknowledged or
the server-enforced deadline expires, a disconnected browser may report only
the bounded reconnect window; it must not claim immediate server deletion or
completed cleanup.

### 4.4 Closing barrier and late-write prevention

At a short `Agents` mailbox linearization point, mark the root and descendants
Closing, revoke access/routes, bump generation, and cancel its token. Keep a
private closing entry holding cleanup handles; do not discard ownership state
merely because the tree has disappeared from visible maps. All admissions,
including lazy session lookup, reject these ids; absence must not mean create.

An explicit backend close replies as soon as this revocation linearizes, with
cleanup pending. It does not wait for owner teardown or claim deletion; terminal
cleanup status is reported separately.

Then an asynchronous teardown task asks the existing owners to end that tree.
Do not block the Agents mailbox waiting on an owner that may itself ask Agents.
Pass the fixed subtree, generation, and cancellation context; return completion
to Agents in a typed message. Owners acknowledge only after they have:

1. rejected new work and resolved approvals/asks/waits/queued intake as closed;
2. cancelled and joined turns, retry loops, title/maintenance tasks and native
   descendants; cancelled every running tree-owned pipe/PTY job, killed its
   process group, drained `jobs/logs` readers/writers, and waited for reaping;
3. invoked browser `Close`, closed isolates, terminated REPL/kernel workers and
   removed `runtime/<session>.monty`, ended app/computer/voice work and frames,
   and removed shell registry entries, learned routes, and owner-local caches;
4. drained artifact, attachment/upload, spill, preview, and other admitted
   writers before removing their memory records and scratch bytes;
5. removed search candidates, communication projections, terminal state, and
   frontend cache entries.

An owner-local serialized commit fence or generation-checked capability gates
both file creation and state/event publication. A metadata check followed by an
unguarded asynchronous write is insufficient. Already-admitted writers must
finish or be joined before acknowledgement. Late callbacks carrying the old
generation are discarded; they may not recreate paths, ids, messages, titles,
or cache entries. This includes tool subprocess completions and deferred title
responses. No content-bearing private tracing (including title hashes).

Only after the cleanup task has awaited every typed owner acknowledgement may it
drop the in-memory database, delete scratch, and ask Agents to publish authorized
final `SessionDeleted`, children before root. Enqueueing cleanup commands is not
completion. `SessionClosing` revokes views immediately. Close may report
`cleanup_pending`; failure reports `cleanup_incomplete` and retains inaccessible
retry state. Never publish final completion or forget retry paths early. Retry
off-mailbox; no generic resource registry. An escaped process is a known
limitation, not a guarantee that every external effect has stopped.

### 4.5 Memory and scratch

Use one single-connection SQLite `:memory:` database per root tree with the same
session/accounting schema and migrations. Set `PRAGMA temp_store=MEMORY`; never
attach disk storage or create a journal, WAL, temp database, spill, backup,
export, or dump. Private retention is not persistent SQLite followed by purge.
Disable automatic knowledge harvest and refuse
`knowledge_put/delete/restore`; reads of existing knowledge are allowed.
Private session context is not indexed into durable memory/search stores.

Create a mode-0700 random directory under the OS user temp location, without
project/title in its name. Owners use subdirectories for attachments, artifacts,
spill, jobs/logs, runtime, browser, computer, and voice data. Propagate
`SessionResources` through ToolCx, Session::Shared, and non-tool owner commands;
clients cannot choose its root. `put_session_attachment` resolves the authorized
session, generation, and resource root before writing or content-addressing any
bytes; the existing global durable attachment store is not a staging area.
Preview materialization, app sources and all secondary caches follow the same
routing. Scratch files may remain after a crash or failed unlink and may be
visible to filesystem snapshots. This is honest best-effort cleanup, not erasure.

A root holds an OS-backed exclusive owner lock for its scratch lifetime. Startup
may remove a recognized private directory only after acquiring its lock and
proving the owner dead; age or PID alone is insufficient. Never follow symlinks
or sweep another live process's roots. If safe lock/ownership proof is unavailable,
leave it for explicit cleanup. A live root older than 24 hours is not stale.


Browser private contexts use an isolated volatile cookie/storage profile, not a
shared durable cookie jar. Learned routes and captured credentials remain in
owner-local private memory; no global route-store write. REPL checkpoints,
compaction snapshots, attachment previews and voice scratch must use the private
root or memory, never their normal durable paths. No private push notification
body is sent through OS/channel history. The first-use warning distinguishes
these harness protections from arbitrary tools explicitly writing outside them.

Lease lifetime follows the concrete owner `RpcPeer`, not “any peer for this
user.” Transport liveness drives its closed signal; do not depend on a browser
unload callback or invent a second heartbeat lease. A transport that cannot
eventually close its peer cannot offer private mode.

## 5. Storage and side-effect checklist

| Owner | Private routing | Closing acknowledgement |
|---|---|---|
| Agent session/history/todo/cost/communication | Single-connection in-memory SQLite | Join tasks, drain writes, then drop database |
| Artifact/app and attachments/uploads | Memory records, target-owned private scratch bytes | Stop calls/download/upload writers, remove records and bytes |
| Spill and previews | Private scratch only | Drain materializers, remove bytes |
| Jobs and `jobs/logs` | Private logs; existing process ownership | Cancel every running subtree job, kill/reap process groups, drain readers/log writers |
| Runtime/Monty and REPL | Private `runtime/<session>.monty` and checkpoints | Terminate workers, remove runtime state |
| Browser | Isolated volatile profile and private files | Invoke `Close`, close isolates, remove profile |
| Shell registry and learned routes | Owner-local memory | Remove entries and routes |
| Computer and voice | Owner-local state and private scratch | Stop workers, drop frames, remove scratch |
| Search/notifications/frontend caches | Authorized memory-only projections | Remove candidates and cached content |
| Knowledge | Read only; no harvest or mutation | No durable writes to undo |
| Explicit external tool effects | Warning; not sandboxed | Not promised undone |

This checklist covers private data from creation, not just deletion. New
session-scoped writers must declare their routing before private mode is complete.

Existing artifact row deletion and asynchronous orphan sweeping are crash
recovery, not a close barrier. The artifact owner acknowledges only after
stopping uploads/downloads/materializers, draining admitted writers, and removing
the private records and scratch bytes. A later sweep cannot justify
`cleanup_complete`.

## 6. Model usage and cost

### 6.1 Native attempt identity and settlement

Each logical request has a `request_id`; every attempt mints one globally unique
`call_id`, persisted before dispatch and reused for every update/replay of that
attempt. `call_id` is the sole primary/idempotency identity. `request_id` groups
attempts and the monotonic attempt ordinal is descriptive, not another key.
Provider request ids are provenance, not local identity. A tool round,
automatic/manual compaction, explicit durable harvest, and title request each
passes through the same accounting seam. `session_cost` reads accounting rows;
it never recomputes totals from visible messages, undo/redo masks, trimmed
history, compacted text, or transient `TurnFinished` totals.

Before transport can run, durably commit a pending attempt row in the Agent
store. Mark it dispatched and commit before invoking transport. If cancellation
or crash leaves dispatch uncertain, preserve unresolved/unknown coverage, not
zero. A conclusively local pre-dispatch failure may become `not_dispatched` and
is excluded from spend/dispatch totals. If reservation or the dispatched marker
cannot commit, do not dispatch. Settlement updates that same attempt with
partial/final observations on success, error, or cancellation, commits before
invalidation/retry, and never deletes the row. Startup marks surviving
pending/dispatched attempts unresolved; it neither loses them nor resends them.

Track title work rather than detaching it beyond the lifecycle. Fence title
writes/events against generation changes and join it at close. Remove private
prompt/reply/title content from diagnostics, including rejected title replies.
Private harvest never dispatches. Other maintenance calls use the originating
session's accounting owner and generation.

### 6.2 Tokens, provenance, money, and coverage

Keep input, output, cache-read, cache-write, and current-context tokens optional.
An existing adapter's normalized estimate may be retained as diagnostic data;
do not build a local pricing engine. Raw provider usage is bounded structured
billing metadata, never prompt text. `chars/4` is context-only. Context is a
latest observation, not a descendant sum.

At adapter ingestion, preserve the normalized monetary observation and origin.
`provider_reported` means an explicit provider/upstream value;
`adapter_estimated` means the adapter derived it. If the event collapses those
into one untagged total, patch that seam rather than infer provenance. Only
`provider_reported` populates `returned_nanos`; only a normalized adapter
estimate populates `estimated_nanos`.

Returned and estimated observations may coexist on a call, but effective
aggregation partitions them: put the call in the returned bucket when returned
is present (including zero), otherwise in the fallback-estimate bucket when an
estimate is present, otherwise mark it unavailable. A retained estimate beside
a returned value is diagnostic only and excluded from every subtotal/total.
Thus no call contributes to both monetary buckets. Arbitrary HTTP/MCP/tool
headers are not billing observations. Frontends only render owner summaries;
they never apply rates, reprice history, or derive money from transcript rows.

Money has an ISO 4217 currency and nonnegative integer nanos of that currency
unit (1 unit = 1,000,000,000 nanos). Validate returned f64 amounts as finite,
nonnegative, convertible within i64; round once at adapter ingestion, retain
source precision/provenance. Invalid currency/amount is unknown with a content-free
warning. Store integer nanos in SQLite; serialize nanos **as decimal strings**
over JSON to avoid JavaScript's 53-bit limit. Sum with checked wider arithmetic;
report overflow as unavailable rather than wrap. Group totals by currency;
never silently convert; standard ACP non-USD currency remains grouped as received.

Summaries contain per-currency returned subtotals and coverage, plus a separate
estimate-for-unavailable subtotal, known/unknown token coverage, unresolved
attempts, history completeness, and remote-meter gaps. No dispatched calls
displays `Returned cost —`. All returned displays `Returned cost USD 0.123`.
Partial coverage displays `Returned cost USD 0.100 · 2 unavailable` and, on a
second line, `Estimate for unavailable calls ~USD 0.023`. Never render
`USD 0.100 + ~USD 0.023` as one apparent total. Imported/old sessions with no
ledger remain historical-cost-unknown, not complete zero. A newly created
session with no calls is distinguished from missing old history. Estimated zero
remains labeled estimated. A remote meter is not mislabeled one provider request.

### 6.3 Standard ACP and remote cumulative meters

ACP `UsageUpdate.cost` is an optional standard cumulative object
`{ amount: f64, currency: String }`; theuth validates the currency string as ISO
4217. This is not an extension or a per-prompt charge. `used` is current
context, not billed input tokens. Consume cost even when token accounting is
unavailable.
OMP's `acp-agent.ts::#emitEndOfTurnUpdates` reads cumulative
`getUsageStatistics()` and emits `usage_update.cost` in USD only when
`contextUsage` exists and cumulative cost is greater than zero. It does not emit
a per-turn delta; absence does not assert zero. Theuth's current
`acp_child.rs` handler stores only `u.used`; the proposed path also consumes the
optional cost.
Store its provenance as `opaque_remote_reported`: useful cumulative telemetry,
not provider-invoiced cost.

A configured remote harness has a meter identity scoped by backend origin,
external session id, and a locally tracked epoch. Persist meter state, its
attributed charge observation, coverage, and root revision **in one transaction**.
For an externally confirmed new session created by this wrapper, baseline is
known zero: a first cumulative 0.20 legitimately contributes 0.20. Persist that
new-session fact before sending prompts. For a loaded/imported session without
an observed initial baseline, establish the first total as baseline and mark
prior/unobserved spend unknown; do not attribute that historical amount as new
theuth spend. Later attributable amount is high-water total minus baseline.

Equal replayed totals produce no additional charge. Monotonic increases update
the same epoch's charge observation, never append the cumulative total as a new
charge. Persist before acknowledging the observation. Reconnect/load resumes
that state. When a lower total arrives, do not subtract or assume it is a new
charge: it may be stale delivery or reset. Retain the previous high water and
flag ambiguous coverage; only a confirmed new epoch/session establishes a new
baseline. An unobserved reset boundary remains unknown. Currency change likewise
requires a distinct meter epoch and marks the boundary unknown. Sequence/report
ids, when a remote provides them, reject replays/out-of-order data; standard ACP
must not be assumed to provide such ids. Stream ordering is preserved locally.

Remote prompt completion without a usable cost observation is unknown, not zero.
A later cumulative observation can cover earlier gaps only within the same
established, nonambiguous baseline epoch. Persist coverage boundaries so reload
does not turn missing observations into complete coverage. If detailed per-call
reports are separately supported, select either those reports or the overlapping
cumulative meter as authoritative, never both. Stable origin/report ids dedupe
such reports atomically; unsupported reports stay unknown.

An opaque remote wrapper owns an **inclusive** charge for its hidden nested
agents. Display that scope explicitly. Do not also sum parent task-result totals
or synthetic child rows for those same charges. Hidden child breakdown is
unavailable, not estimated. Native descendants with independently disjoint calls
remain separate accounting owners. Without proof that a remote breakdown is
disjoint, prefer the inclusive meter once.

### 6.4 Durable lineage and schema

Delivered migration `0015_session_lifecycle.sql` (0014 belongs to 52):
the Agent store owns `cost_owners`, minimal billing-attribution nodes rather than
another session registry. Their ids match session ids, but they deliberately have
**no FK to live sessions**. Deleting conversation rows cannot cascade spend away.
`parent_id` and `root_id` are immutable accounting lineage; only live session
metadata supplies project.

```sql
ALTER TABLE sessions ADD COLUMN project TEXT NULL;
CREATE TABLE cost_owners (
  id TEXT PRIMARY KEY,
  root_id TEXT NOT NULL,
  parent_id TEXT NULL REFERENCES cost_owners(id),
  deleted_at TEXT NULL,
  history_coverage TEXT NOT NULL,
  revision INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX cost_owners_root ON cost_owners(root_id);
CREATE INDEX cost_owners_parent ON cost_owners(parent_id);
CREATE TABLE model_calls (
  call_id TEXT PRIMARY KEY,
  owner_id TEXT NOT NULL REFERENCES cost_owners(id),
  request_id TEXT NOT NULL,
  attempt INTEGER NOT NULL,
  turn_id TEXT NULL,
  kind TEXT NOT NULL,
  status TEXT NOT NULL,
  provider TEXT NULL,
  requested_model TEXT NULL,
  served_model TEXT NULL,
  provider_request_id TEXT NULL,
  input_tokens INTEGER NULL,
  output_tokens INTEGER NULL,
  cache_read_tokens INTEGER NULL,
  cache_write_tokens INTEGER NULL,
  context_tokens INTEGER NULL,
  returned_nanos INTEGER NULL CHECK(returned_nanos >= 0),
  returned_currency TEXT NULL,
  returned_source TEXT NULL,
  estimated_nanos INTEGER NULL CHECK(estimated_nanos >= 0),
  estimated_currency TEXT NULL,
  estimate_source TEXT NULL,
  unknown_reason TEXT NULL,
  coverage TEXT NOT NULL,
  source_metadata TEXT NULL,
  created_at TEXT NOT NULL,
  dispatched_at TEXT NULL,
  settled_at TEXT NULL
);
CREATE INDEX model_calls_owner ON model_calls(owner_id);
CREATE TABLE remote_meters (
  id TEXT PRIMARY KEY,
  owner_id TEXT NOT NULL REFERENCES cost_owners(id),
  origin TEXT NOT NULL,
  external_session TEXT NOT NULL,
  epoch INTEGER NOT NULL,
  source TEXT NOT NULL,
  currency TEXT NOT NULL,
  baseline_nanos INTEGER NULL CHECK(baseline_nanos >= 0),
  high_water_nanos INTEGER NULL CHECK(high_water_nanos >= 0),
  attributed_nanos INTEGER NULL CHECK(attributed_nanos >= 0),
  baseline_kind TEXT NOT NULL,
  scope TEXT NOT NULL,
  coverage TEXT NOT NULL,
  coverage_boundary TEXT NULL,
  last_report_id TEXT NULL,
  source_metadata TEXT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE(origin, external_session, epoch)
);
CREATE INDEX remote_meters_owner ON remote_meters(owner_id);
```

Application validation pairs each amount with currency and source, requiring
`returned_source = provider_reported` for native returned values and
`estimate_source = adapter_estimated` for native estimates;
all three fields are absent together. Remote meters require
`source = opaque_remote_reported`. Validation also checks the enum-like status,
kind, coverage, and scope strings. `model_calls` records native attempts;
remote meter accounting is separate, not fake native call rows. Remote coverage
boundaries include the last covered local prompt ordinal and unresolved ranges,
encoded in bounded structured metadata; per-prompt remote state already belongs
to the ACP wrapper. Never retain an unbounded copy of remote update payloads.
Source metadata identifies adapter version and source precision without
credentials or model content. Optional per-call remote reports use their stable
identity as `model_calls.call_id`; summary selection excludes any overlapping
cumulative meter. Standard ACP needs no such extension.

Backfill accounting owners for old live sessions with unknown historical
coverage. Create each new owner atomically with its session; new owners start
complete-since-creation. Reservation, settlement, meter, unknown-coverage,
topology, or deletion changes increment the affected owner and every live
ancestor revision in the same transaction as the ledger mutation. Queries use a
consistent read transaction; invalidation carries every changed live revision.

Deleting a durable child tombstones its accounting owner and keeps lineage and
ledger, while removing its conversation. Recursive totals traverse `cost_owners`,
not only surviving `sessions.parent`. The UI can show Deleted child with its
retained cost; do not preserve transcript/title merely for accounting. Root
explicit deletion removes all calls/meters and owners in that root in one
transaction (owners child-first), even previously deleted descendants. Private
close drops the whole memory pool. Rewind/edit/clear/compaction do not erase spend.
Fork copies context/conversation but no calls/meters/owners except a fresh root
owner; copied context tokens are not recharged. Costs from the source tree remain
there. Root deletion, unlike child deletion, intentionally removes its ledger.

### 6.5 Query and numerical invariants

`session_cost` returns direct, descendants, total, immediate children with their
recursive totals, scope/provenance/coverage, and an opaque revision. Deleted
child costs remain reachable only via the authorized surviving tree, not through
resurrected sessions. Query recursion uses retained accounting ancestry. The
frontend displays these summaries; neither events nor task results are charges.

Worked example: root `R` has direct USD 0.10, child `C` has direct USD 0.20,
and nested child `G` has direct USD 0.30. Therefore `G.total = 0.30`,
`C.total = 0.20 + 0.30 = 0.50`, and
`R.total = 0.10 + C.total = 0.60`; `R.direct` remains USD 0.10. Replaying the
same settlement or its invalidation event leaves three charge rows and every
subtotal unchanged. Deleting `G` or `C` conversation rows still leaves
`R.total = 0.60`. A fork starts with no charges. Root deletion removes all
USD 0.60 from that root's ledger.

For an opaque remote child whose cumulative inclusive total is USD 0.50,
root direct USD 0.10 gives root USD 0.60, not USD 1.10. Replayed 0.50 contributes
nothing new. A later 0.60 gives root 0.70; hidden descendants are not extra rows.
An imported first observation of 0.50 establishes baseline with prior cost
unknown, not a new 0.50 charge.

### 6.6 Backend integration boundary

`crates/agent/src/backend.rs::Backend::stream` currently returns
`genai::chat::ChatStreamEvent`; that transport carries no provenance-tagged
monetary observation into theuth today. Preserve cost before adapter normalization
drops it. Extend the adjacent stream/usage result narrowly; if rust-genai itself
discards the provider value/source, patch the existing rust-genai fork already
selected in `Cargo.toml`. Do not add another HTTP client/interceptor or reprice
in a UI. Adapters label a normalized amount provider-reported or estimated;
unsupported providers remain unknown unless their adapter already supplies an
explicit estimate. V1 adds no local pricing engine. Acceptance follows real
transport → observation → ledger → owner summary, never a frontend-injected value.

## 7. Authoritative PTY observation

### 7.1 Owner and snapshot

`Jobs` owns the existing 120-column × 40-row `vt100` cell state per PTY.
Increase normal-buffer history to at most 512 lines; alternate-screen content
does not populate normal scrollback. Snapshot extraction is a short clone under
the parser lock; redaction, rendering, and RPC serialization happen after
release. No frontend emulator, second parser, raw escape replay, or job-control
change is needed.

Add `JobView.session`; add `name` and `pty` to `JobEvent::Started`.
`list_terminals(session, descendants)` resolves the authorized session subtree
server-side and returns only retained `pty` jobs with owning session/job id,
name, state, 120×40 dimensions, daemon epoch when available, and latest revision.

`pty_screen(session, job, viewport, if_revision)` returns `Updated` with an
immutable bounded page, `Unchanged`, or `Unavailable`. An updated page contains
identity, daemon epoch when available, generation/revision, exact viewport
identity, page origin, retained-history count, truncation/eviction and
`redaction_complete` flags, styled cell runs, cursor position/visibility,
alternate-screen state, line-wrap flags, fixed dimensions, and final exit state.
Cells carry text, fg/bg colors, supported attributes, and width/continuation
markers for wide glyphs.

Viewport identity includes retained-line anchor or live screen, offset, and
height. `Unchanged` may omit the screen only when `if_revision` matches and it
echoes that exact viewport identity; `screen: null` without matching
`(anchor/live, offset, height)` is invalid. Scrolling therefore returns the
requested page even when the terminal revision itself has not changed.

The default live page is the current 40 rows. History pages contain at most 80
rows, 120 cells each. Retain cell positions, wide-character continuation,
combining glyph associations, and wrap markers; colors/attributes are structured
values, never ANSI. A late attach reads current styled/redacted cells and bounded
history without a start event. After job/history eviction or an unknown id,
return `Unavailable` and render “No longer available.” Do not infer daemon
restart from absence alone. Only a known request epoch differing from a known
current daemon epoch may render “Daemon restarted.” Logs never reconstruct a PTY.

### 7.2 Redaction before serialization

Vault-known literal redaction is best effort, not a guarantee against arbitrary
encoding, transformation, or secrets the vault does not know. Build logical
text across style boundaries and soft wraps for the bounded retained buffer,
then redact before slicing the requested page. Include context outside both page
edges of at least the longest known literal minus one cell, capped by retained
history; never match only the requested page, style run, or physical row.

If history eviction or truncation removes context needed to decide a possible
cross-boundary match, set `redaction_complete = false` and identify the retained
boundary; never claim complete redaction. Mask each matched cell with neutral
placeholders while preserving its width/continuation markers, row geometry, and
cursor alignment; suppress associated style/link metadata. Never send raw hidden
text, ANSI/OSC/DCS, clipboard OSC 52, OSC 8 URLs, image payloads, terminal query
replies, byte logs, or arbitrary parser metadata. Sanitize control characters
and metadata strings. Share this stronger scrubber with cards/model screen tools
where possible. Private snapshots obey owner access and memory-only cache rules.
Copy/selection copies the redacted representation and exposes incomplete status.

### 7.3 Bounded retention and backpressure

Keep the existing bounded reader channel and coalesced hints (at most one per
500 ms per job). Add one global Jobs-owned byte budget: default 64 MiB for
retained snapshot/history payloads across all PTYs, including run metadata.
The existing 32 retained job rows per session is not a daemon memory bound:
sessions are unbounded and each PTY may approach roughly 2 MiB. The global byte
budget covers all sessions and is authoritative over per-terminal history caps.
512 is a per-terminal maximum, not a promise of retention under pressure.
Evict exited history first, then oldest normal scrollback. Preserve live screens;
if the budget cannot reserve another live parser's bounded screen, reject the
new PTY with a clear capacity error rather than grow without bound. Cap text per
cell at 64 UTF-8 bytes (replace pathological combining overflow), and count actual
retained allocations plus conservative overhead. This bounds pathological cells,
not just row counts. Account immutable snapshot clones under the same owner
budget; no per-revision snapshot archive.

A serialized response is capped at 256 KiB. Page fewer rows when style/run
fragmentation reaches that cap; return the next anchor and truncation state.
Admission limits concurrent snapshot clones in Jobs; excess asks coalesce for
identical keys or receive retryable busy, never an unbounded render queue. Job
output parsing/control must not wait on slow observers. Clients never hold the
parser lock. These are Jobs-local limits, not a terminal broker.

While a viewer is open and visible, use the BrowserLive pattern: call
authoritative `pty_screen` immediately, then poll every 500 ms. Add no terminal
subscription or stream. Existing coalesced `JobScreen` hints may wake the same
scheduler early, but correctness and late attach depend only on polling.
At most one request is in flight per view and at most one starts per 500 ms
after initial open; mark dirty during flight and fetch latest when eligible.
Hidden or closed viewers stop polling. Compact cards may remain event-driven.

Define `TerminalKey = (harness, session, job)`; revisions are scoped to that key
and never ordered across keys. Each ask captures
`QueryKey = (TerminalKey, viewport(anchor/live, offset, height))` and the current
`selection_epoch`; increment the epoch whenever `TerminalKey` changes. Apply a
reply only when its query key and epoch still match and its revision is not older
for that same terminal. A higher revision from an old terminal is irrelevant and
must not paint the new selection. `screen: null` also requires the exact viewport;
revision equality alone is insufficient. Re-list/poll on lag, reconnect,
foreground resume, exit, or harness switch. Text, style, cursor, wrap, history,
screen mode, and final state increment revision; retain final state until eviction.

### 7.4 Clients

TUI `/terminals` and PTY cards open a read-only overlay with owner/job state,
monospace styled cells, visible cursor, 120-column horizontal pan, and bounded
history scrolling. Escape closes it without releasing the private owning tree.
No keystroke is forwarded to the process unless the viewer is in its
interactive mode (55 §3: `i` enters, Ctrl-] leaves); manual refresh re-asks the owner.

Shared React desktop/mobile uses a responsive Dialog/Sheet like BrowserLive,
terminal picker, fixed-cell grid with styled spans, accessible status text,
text selection, and 44-pixel touch targets. Mobile uses a full-height sheet.
Clients that cannot render a color or attribute ignore that style and render the
same redacted cell text in the grid; plain degradation never falls back to ANSI.
A child link stays within the tree. Input is 55 (`pty_input`); signal, cancel,
resize, and takeover controls are out of scope. Existing model tools retain their job-control powers.

## 8. Contracts and frontend integration

### 8.1 One command surface

Add these methods to `rpc::Api`; parameter/result types belong to the owner
plugin and get generated TypeScript bindings. Preserve existing
`new_session() -> SessionId`, `send_input(None, ...)`, and
`put_attachment(bytes, ext)` exactly as durable-only methods. `create_session`
is additive, always requires `CreateSessionOptions`, and always returns
`SessionOpen`; omitted arguments never select a different return shape.

```text
create_session(options: CreateSessionOptions) -> SessionOpen
resume_private(session, view, lease_secret) -> SessionOpen
leave_session(session, lease_secret) -> CloseResult
set_session_project(session, project?) -> ()
session_cost(session) -> SessionCost
put_session_attachment(session, bytes, ext) -> PathBuf
list_terminals(session, descendants?) -> Vec<TerminalSummary>
pty_screen(session, job, viewport, if_revision?) -> PtyScreenReply
pty_input(session, job, text?, keys?) -> ()          // 55
```

Private lifecycle/access methods obtain the authenticated `RpcPeer` from RPC
extensions, never caller-supplied peer ids. TUI supplies one stable in-process
peer and explicitly leaves on navigation/quit; do not mint one per call.
`SessionOpen` contains session/root ids, retention, and (private only) a reconnect
secret. `CloseResult` distinguishes logical revocation from cleanup completion.
Secrets are redacted from tracing and never become model-visible tool results.

Add effective `meta.project`, retention, and accounting summaries/revisions to
session load/list and agent rows. New fields use compatibility defaults. A
private upload must follow private creation and use the session-scoped method,
which resolves target ownership before any byte write or content addressing;
legacy globally addressed uploads must never be staged then moved into private
storage.
Existing clients cannot create private sessions accidentally.

Owner events:

```text
SessionMetadataChanged { root, project }
SessionClosing { root, generation }
SessionDeleted { session }
ModelUsageChanged { session, revision }
```

Money events invalidate, never add deltas. After commit, publish
`ModelUsageChanged { session, revision }` for the charged owner and separately
for each live ancestor, using each new revision, so every screen re-reads its
authoritative summary. Reconnect
and `Lagged` also trigger authoritative reads. Private events, including
existence-only metadata, go solely to the owning view. Final deletion uses the
captured owner route; it must not require a deleted lookup for authorization.
Resolve that route before enqueueing; never broadcast private
list/title/project/cost/lifecycle events for clients to discard.

### 8.2 Owner coordination, not god-wiring

Agent owns retention, accounting and the close task. Tools owns Jobs and its
termination commands. Agent defines the lifecycle command/reply messages but
does not depend on higher plugin crates. At composition, the binary passes typed
close-and-ack recipients from artifact/app/browser/computer owners into Agents,
using the existing upper-layer registration direction and avoiding a dependency
cycle. This fixed participant set is not a callback/resource registry. Disabled
owners are known absent; an enabled owner that fails to register blocks private
creation. Cleanup runs off-mailbox and returns typed completion to Agents.

`SessionResources` needs to cross the existing tool execution contract; keep its
minimal root/retention/generation capability alongside `ToolCx`. It is not a
client-set path or global filesystem routing service. Commit fences remain
owner-local. Shell/registry/route caches and voice work are included in their
owners' acknowledgements, not forgotten because they lack a visible panel.

### 8.3 UI placement

TUI: `/new private` uses explicit private creation; `/project <label>` and
`/project off` set or clear root metadata. `/sessions` and `/artifacts` group by
project. The header shows the authoritative inclusive `session_cost` summary;
`/cost` opens direct, children, deleted-agent, source and unknown-coverage
details. `/agents` shows each child's own inclusive cost. No frontend sums
assistant/task transcript entries or applies model rates. `/terminals` opens the
observer described above.

Desktop/mobile: New Chat has a Private option, a project picker populated from
existing keys plus free entry, and the retention warning. Session menus offer
Move to project/Remove from project. The shared sidebar and artifact explorer
group by project. A cost pill beside context opens the same breakdown; keep the
amount visible on narrow screens. Agent rows show their recursive total. Use
one router-level private-owner coordinator so late route unmounts cannot close
a newly opened workspace. Private UI state clears immediately on local leave;
if a leave cannot reach the server, explain that backend expiry is the fallback.
Backgrounding is not intentional navigation; an undetectable dead connection
requires transport liveness detection before the 30-second grace can begin.

Slack/Signal and unattended schedules stay durable; never forward a private
transcript or create private bindings implicitly. Text channels can show a cost
summary for durable sessions and link to an existing terminal viewer where
supported, but do not stream terminal output into channel history. ACP as a
frontend stays durable. Outbound standard ACP cost can represent only one
currency and no completeness/provenance label: emit it only for a complete,
single-currency returned total; omit rather than misrepresent mixed/unknown
cost. PTY input, new permissions, and provider spending limits are not added.

## 9. Migration, delivery, and acceptance

Existing sessions migrate with `project=NULL`, accounting lineage backfilled,
and historical cost coverage explicitly unknown. No transcript reconstruction,
repricing, or retroactive estimate fills that gap.
Private pools run the same schema in memory. Old clients keep the exact durable
creation/upload methods. Contract
checks require options and a `SessionOpen` result from additive `create_session`;
omitted arguments are invalid. New wire fields are additive, and private mode
appears only when every retention participant is ready.

Deliver in owner-sized commits, each with a runnable check:

1. **Project metadata.** Migrate a copy of an old database, create two roots,
   child sessions and artifacts, set project through a child, and verify one
   root update emits `SessionMetadataChanged` and regroups all descendants and
   artifacts without artifact row rewrites. Check 127/128/129-byte UTF-8
   boundaries after Unicode trim, every control-character rejection, case
   distinction, fork independence, effective `meta.project`, and that only the
   dedicated nullable column/setter exist—no metadata envelope or project
   subsystem.
2. **Native accounting.** Capture explicit provider-reported cost through the
   real normalized adapter seam, not a mocked UI. When the same call also has an
   adapter estimate, assert returned contributes only to the returned bucket and
   the retained estimate is diagnostic, excluded from every subtotal. With no
   returned value, the estimate enters only the fallback-estimate bucket; with
   neither, the call stays unavailable. Verify two rounds, failed retry,
   compaction, harvest and title have distinct `call_id`s. Kill between
   reservation/dispatch/settlement; pending coverage survives restart. Each
   settlement invalidates the charged owner and live ancestors. Load an old
   session without ledger rows and assert historical coverage unknown without
   transcript/UI repricing.
3. **Remote/ancestry accounting.** Replay cumulative ACP 0.20, 0.20, 0.35 and
   assert 0.35 once. Test imported unknown baseline, lower/stale totals,
   confirmed resets, omitted zero, EUR, invalid values and restart. Exercise
   `R=0.10`, `C=0.20`, `G=0.30`: assert `G.total=0.30`,
   `C.total=0.50`, `R.total=0.60`, and replay idempotence. Undo, trim, compact,
   and delete child conversation rows; totals remain ledger-derived. Never sum
   an opaque remote total and its overlapping breakdown. Confirm returned zero
   excludes a nonzero diagnostic estimate from displayed estimate subtotals.
4. **Private storage and admission.** Use a unique canary in a private turn,
   native child, app/artifact, upload, spill, screenshot, Monty runtime, browser,
   shell route, computer, voice, and PTY. Confirm the session/accounting schema
   runs through one SQLite `:memory:` connection with memory temp storage and no
   database, journal, WAL, attachment, or spill file. Inspect durable tables and
   normal attachment/artifact/spill/runtime/job/browser/computer/voice paths,
   search, logs, route registries, and client persistence for the canary.
   `put_session_attachment` must resolve private target ownership before its
   first byte write; the global content-addressed store remains untouched.
   Scratch is expected only in the private root and carries the explicit
   crash/snapshot caveat. IndexedDB retains only link harness identities and
   `localStorage` only theme; no private restoration id, draft, file cache,
   preview, rendered content, or push payload persists, and leave clears all
   in-memory private state. Verify no automatic knowledge writes, external child
   dispatch, durable route, channel binding, or schedule creation. Where a
   provider supports a no-store option, verify it is sent while the UI still
   disclaims control over provider retention.
5. **Private lifecycle.** Bind a root-local view to the existing stable
   `RpcPeer`, then test explicit navigation during a running turn, title callback,
   pending approval, artifact/upload write, and running pipe/PTY jobs. Root,
   child, PTY, and artifact subviews retain ownership; another root or owning-view
   close releases it. `RpcPeer` closure starts the server's fixed 30-second
   grace; activity by another authenticated peer/user does not extend it.
   Reconnect the same view with the valid secret, then prove stale peer messages
   cannot close the rebound owner. An acknowledged private close revokes
   admission before reply, cancels every subtree job, kills/reaps process groups,
   invokes browser `Close`, and drains runtime/Monty, artifact/upload/spill,
   shell/route, computer, voice, log, and cache owners before completion. It
   never uses ordinary durable deletion's reject-running rule, and artifact row
   deletion/orphan sweep alone never satisfies the barrier. A second
   authenticated client cannot list/read/subscribe/download or retain the tree
   and receives no session-list, title, project, cost, closing, or deletion
   event for it. On disconnect the client shows only the bounded reconnect
   deadline, never immediate deletion. Expiry cannot resurrect. Fault one owner:
   report incomplete cleanup and retain retry handles, never claim deletion.
   Restart removes only provably dead scratch roots, not a live second harness's
   directory.
6. **Terminal authority.** Start PTYs in a root and nested child; verify
   `JobView.session`, `Started.name/pty`, and subtree discovery. Attach late from
   TUI and desktop/mobile: immediate/500 ms `pty_screen` polling recovers the same
   redacted 120×40 cell window without a new subscription. Verify styled runs,
   plain unsupported-style degradation, and absence of input/signal/resize
   controls. Exercise alternate screen, colors, wide/combining glyphs, history
   eviction, exit, duplicate job ids across sessions, lag, reconnect, and the
   one-in-flight limit. Switch from `j1` to `j2` while `j1` is in flight, then
   return `j1` with a higher revision; it must not paint `j2`. For the same
   terminal/revision, change `(offset, height)` and reject unchanged/null without
   exact viewport identity. Split known vault
   literals across chunks, styles, soft wraps, and requested-page edges; assert
   fixed-width masks and no complete literal. Evict required boundary context
   and assert `redaction_complete = false`, without claiming arbitrary-transform
   protection. Fill 512-row PTYs across many sessions beyond 32 retained jobs;
   the daemon-wide byte cap must evict or reject without growth. Unknown/evicted
   jobs render “No longer available”; only known differing epochs render
   “Daemon restarted.”
7. **Client parity.** Exercise TUI in-process Api and desktop WebSocket plus
   mobile Link. Observe identical project/cost/terminal state. Suspend/resume
   mobile, navigate away privately, and verify cache clearing and truthful
   disconnect/cleanup status. Check keyboard navigation, screen-reader status,
   selection, 44-pixel targets and 120-column pan on an 80-column terminal.
8. **Documentation.** Run owner tests, generated-binding checks and the manual
   scenarios above. Record actual results before changing the amendment in 00
   or this document to as-built. A doc-only change does not claim those tests ran.

## 10. Alternatives and remaining gates

- **Persist then delete private history:** rejected; WAL/backups outlive rows.
  Memory history plus explicit scratch caveat is a narrower, honest promise.
- **Any viewer keeps private alive:** rejected; another device must not defeat
  the creator's leave. Sharing requires a later explicit design.
- **Instant deletion on socket loss:** rejected; phone reconnects are routine.
  Explicit leave is immediate; unexpected loss has bounded grace after detection.
- **Project entity/config/workspace:** rejected; grouping needs one metadata key.
- **Cost from visible transcript or frontend sums:** rejected; compaction,
  maintenance, retries, forks and inclusive child rows invalidate that model.
- **Estimate labeled returned charge:** rejected. An external harness's returned
  amount remains an opaque remote report, not audited billing.
- **New terminal broker/raw byte stream:** rejected; Jobs already owns parsed
  state. Browser-style snapshots give late attach and recovery with less state.
- **Human PTY control/resize:** deferred; requires controller arbitration,
  provenance and mobile key handling beyond reliable observation.

Before implementation claims completion, spike (a) cost preservation through the
patched genai adapters and (b) vt100 cell/history bounds and styled TUI rendering.
Use the dependency's public APIs first; if a bounded-history/combining-cell
requirement needs a patch, document the small patch rather than write a second
emulator. Private mode must remain unavailable until the retention audit passes;
there is no silently degraded private setting. This document makes proposed
choices, including 30 seconds, 512 lines and the Jobs byte budget; they are not
claims about current code or irreversible user-approved product policy.

## Appendix A. Inspected source map

Paths are relative to `~/Develop/theuth`; symbols are preferred because lines move.

- Architecture: `docs/design/00-architecture.md` §§1,4,7,8;
  `docs/design/12-course-correction.md` §§0,7.
- Sessions/store: `crates/agent/src/lib.rs::{SessionSummary,Loaded,Usage}`;
  `crates/agent/src/store.rs::Store::{create_session,descendants,delete_session}`;
  `crates/agent/src/supervisor.rs::Message<DeleteSession>` around 3856.
- Private storage: `crates/agent/src/{repl,spill,loop_}.rs`;
  `crates/artifact/src/{lib,store}.rs`; `crates/app/src/actor.rs`;
  `crates/rpc/src/api.rs::{put_attachment,subscribe_events}`.
- Lease: `crates/link/src/types.rs::RpcPeer` identity/closed signal around
  187–248; `crates/rpc/src/lib.rs` connection extensions around 121–177.
- Cost transport: `crates/agent/src/backend.rs::Backend::stream`;
  `crates/agent/src/loop_.rs::Turn::call` and 2587/2907/3685 paths;
  `crates/models/src/lib.rs::Cost`; rust-genai fork selection in `Cargo.toml`.
- ACP: `crates/agent/src/acp_child.rs` ~687 reads `UsageUpdate.used`; pinned `v1/client.rs` 497–568 defines cumulative `Cost { amount: f64, currency: String }`; `crates/rpc/src/acp.rs` emits updates.
- PTY: `crates/tools/src/pty.rs::{PtyHandle::screen,screen_text}`;
  `crates/tools/src/jobs.rs::{Jobs,JobView,JobEvent,Log::push}`.
- Frontends/recovery: `crates/tui/src/{model,ui,lib}.rs`;
  `crates/desktop/ui/src/{components/chat/BrowserLive.tsx,routes/c.$session.tsx}`;
  `crates/desktop/ui/src/components/sidebar/{SessionList,AgentTree}.tsx`;
  `crates/desktop/ui/src/{components/artifacts/List.tsx,App.tsx,store/reduce.ts,rpc/client.ts}`;
  `crates/rpc/src/bridge.rs`. Durable client state found: link identities in
  IndexedDB (`App.tsx:291`, `link/harnesses.ts`) and theme in `localStorage`;
  no transcript persistence or replay was identified.

## Appendix B. Oh My Pi examples to reuse—and not copy blindly

Paths are relative to `~/Develop/oh-my-pi`.

1. `packages/ai/src/providers/openai-shared.ts::applyProviderReportedCost`
   around 414–440 validates returned cost; reuse precedence, retain provenance.
2. `packages/catalog/src/models.ts::calculateCost` shows pricing complexity;
   theuth v1 does not port it and keeps only adapter-normalized estimates.
3. `packages/coding-agent/src/session/session-manager.ts::{entryUsage,addUsage}`
   around 339 aggregates assistant, `model_usage`, and task-result usage.
4. The same file's `appendModelUsage` around 2735 captures off-transcript work;
   `#resetInheritedUsageCost` around 3335 prevents fork charge inheritance.
5. `packages/coding-agent/src/modes/components/agent-hub-projection.ts` around
   63–105 demonstrates parent/child double-count avoidance.
6. `packages/coding-agent/src/modes/acp/acp-agent.ts::#emitEndOfTurnUpdates`
   around 2165–2178 emits cumulative cost only with context and positive cost.
7. `packages/coding-agent/src/modes/components/status-line/segments.ts::costSegment`
   around 583 consumes aggregate cost; `footer.ts` around 175 is the older
   assistant-only pattern not to copy.

## Status

As built 2026-09-19 on branch `session-lifecycle`; commits `6c63f74`, `4738eb3`,
and the wave-3 commit. The design body above is unchanged from the 2026-09-18
draft; deviations are listed below rather than rewritten in.

Verification (orchestrator, 2026-09-19):

- `cargo check --workspace --all-targets`: 0 errors, 0 warnings.
- `cargo test --workspace --lib`: 944 passed, 0 failed.
- `cargo test -p rpc --features ts export_bindings`: ok; `bindings/methods.json`
  lists `create_session`, `resume_private`, `leave_session`,
  `set_session_project`, `session_cost`, `put_session_attachment`,
  `list_terminals`, `pty_screen`.
- Desktop: `tsc -b --noEmit` clean; vitest 29 files / 170 tests pass.
- No LSP servers configured; cargo and tsc are the diagnostics.
- Migration is `crates/agent/migrations/0015_session_lifecycle.sql`; schema as
  §6.4 plus a `cost_owners` backfill for existing sessions with
  `history_coverage='unknown'`.
- Cost worked example (§6, R/C/G 0.60/0.50/0.60) and replay tests pass; a real
  local SSE transport test asserts returned USD 0.25 → ledger → `session_cost`.

Built: §3 (`normalize_project`; effective project via root CTE on every summary;
fork copies once; `SetSessionProject`/`SessionMetadataChanged`; TUI and desktop
grouping/menus). §6 (`cost.rs` Money nanos-as-string; `store/accounting.rs`
reserve→dispatch→settle with per-attempt `call_id`, `request_id` per
`Turn::call`, kinds round/compaction/harvest/title; `ModelUsageChanged` to owner
and live ancestors; genai fork `e65f7e3` carries
`Usage.cost: Option<UsageCost{amount,currency,source}>` keyed on raw provider
`cost`; ACP cumulative meter with known-zero baseline at connect and imported
baseline otherwise; outbound ACP cost only when complete single-currency;
startup unresolve). §7 (vt100 scrollback 512, styled runs, cross-wrap/style
redaction spans with `redaction_complete`, 64 MiB Jobs budget evicting exited
PTYs first then rejecting, 4-permit snapshot semaphore, 256 KiB page cap,
`JobView.session`, `Started{name,pty}`, `list_terminals`/`pty_screen`; TUI
observer and desktop `TerminalViewer` with 500 ms polling, one in flight,
`TerminalKey`/epoch/viewport reply guard; daemon epoch is always `None` today so
"Daemon restarted" never renders). §4/§5 (`create_session`/`resume_private`/
`leave_session`; one `PrivateOwner` = view + `RpcPeer` id + closed token +
generation + secret; 30 s grace watcher; closing barrier: mailbox-linearized
revoke + `SessionClosing` → off-mailbox `CloseTreeAll` through the tools
`ToolRegistry` participants hub (artifact, app, browser, computer, voice,
routes, jobs) with 30 s per-participant timeout → drop in-memory Store +
Scratch → `SessionDeleted` children-first; lazy-create gate; ACP/node/remote
children refused before dispatch; per-tree in-memory SQLite (1 connection,
`temp_store=MEMORY`, journal MEMORY); 0700 flock'd scratch under the temp dir,
sweep only after acquiring the lock; artifact in-memory state per root with
writer drain; Jobs `CloseSessions` kills/reaps/removes logs; routes private in
memory; knowledge mutations refused and auto-harvest/hints off privately; RPC
per-peer `Authorize` on every session method, private event filter with a
process-global owner map, `get_attachment` fails closed for private scratch;
desktop memory-only private state, router-level coordinator, reconnect banner,
grep proof of no new persistent storage; TUI `/new` private + explicit leave).

As built — deviations:

1. The close-barrier hub lives in `tools::ToolRegistry`
   (`RegisterLifecycle`/`ExpectParticipants`/`ParticipantsReady`/`OpenTree`/
   `CloseTreeAll`), not in agent, because browser/computer/voice/routes depend
   on tools but not agent. Agent still defines and owns `CreateSession` etc.;
   `main.rs` sends `ExpectParticipants` for enabled owners.
2. A private fork shares the source tree's in-memory Store and lease (same
   owner) instead of minting a new secret; teardown closes the pool when the
   last tree drops it.
3. The title-task fence is the session's `closed` token (cancelled in
   `Session::on_stop`) rather than a generation compare.
4. The RPC private owner map is insert-only (UUIDs are never reused).
5. No HTTP attachment route exists in `crates/rpc`, so `Cache-Control:
   no-store` was not applicable; JSON-RPC `get_attachment` requires the owning
   session for private paths.
6. Browser (obscura) is an in-memory engine with no on-disk profile; "isolated
   volatile profile" is the per-session isolate. Only screenshots touch disk,
   under the scratch root.
7. `put_session_attachment(session, bytes: base64 string, file_ext)`.
8. The ACP zero-baseline row is USD; a later non-USD meter opens an imported
   epoch.
9. `Turn::knowledge()` also disables knowledge hints (reads) in private
   sessions.
10. Quiet REPL shell jobs have no row and are bounded by the job timeout rather
    than `CloseSessions`.
11. Manual acceptance scenarios §9.1–9.7 were not executed end-to-end on real
    devices/harnesses; the automated unit/integration tests listed above ran.
