# Potential features

Short ideas, not designs. An entry says what is missing, the shape that
fits the harness, what it reuses, and the risk that would sink it. One that
earns a build gets a numbered doc in `design/` and a pointer here; one that
is argued down moves to *Declined* with the reason, so it is not proposed
twice. Shelved full designs (15, 40) stay in `design/` and are listed at the
bottom.

## ask_person — the human boundary as a tool

**Gap.** Every task that touches another human dies at the boundary. "Ask
Alice if Thursday works, then book the room": the model can book a room but
cannot ask Alice; it asks the owner to ask Alice. `message_user` (24) and
`ask_user` reach exactly one person, the owner, while the harness already
has the apparatus for reaching others — a Slack app with `allowed` users and
channels (21), a Signal account with `/signal allow` (09), threads, Block
Kit, reactions.

**Shape.** `ask_person{ who, question, options?, deadline? }`, `Risk::Mutating`.

- `who`: a Slack user id or handle, a Signal number, or a name resolved
  through Slack `users.list` (cached at start). No contact book of our own;
  knowledge concepts already hold "Alice = U0123".
- Sends a Block Kit card (Slack) or one text (Signal) in a new DM thread the
  harness owns. `options` render as buttons, else free text. The card says
  who is asking on whose behalf — fixed text, not configurable.
- **Returns at once as a job stub** — 14's pattern, as `shell` past
  `job_grace_secs`. The turn ends; the reply arrives as
  `<person_reply from="alice" asked="…">` and wakes the session like
  `<job_result>`. `deadline` expiry wakes it with `no answer`, as
  `ask_user`'s timeout does.
- The approval card shows recipient and verbatim text before anything is
  sent. `Risk` is the whole contract (00 §1.7): under `/approvals off` or an
  `approvals: false` schedule it sends unattended; the prompt block says
  when that is appropriate, as 33 §7 does for loops.

**Reuses.** `ask_user`'s actor (a question with options, a timeout, one
answer); `Jobs` ("a result later wakes the session"); Slack/Signal delivery
and inbound routing by thread. New: one tool, one `asked` row keyed by
(channel, thread) so an inbound reply routes to the asking session instead
of opening the responder's own, and the wake. ≈ 200 lines across `agent`,
`slack`, `signal`.

**Value.** Converts the largest class of "cannot finish" into "finishes
later": scheduling, confirmations, hand-offs, on-call questions. With
schedules it gives follow-up ("if Alice has not answered by 3 pm, ask
Bob" = `deadline` + a second call); with subagents, a coordinator: N stubs
out, N replies wake the parent, the parent reconciles.

**Risk.** The reply is text from a non-owner entering the owner's session —
the same thing a `slack.allowed` user's thread already is; nothing new in
kind, but it widens who can type into a session and 28 is not built. Spam:
the gate is per message and the card shows the text.

Status: idea · 2026-09-17.

## Declined

| idea | why |
|---|---|
| Forked subagents (`spawn_subagent{inherit}` = `store.fork` with `parent` set; the child starts on the parent's cached prefix) | Subagents exist to isolate context. A fork hands the child 100k tokens it mostly does not need and pays the attention cost on every round; the cache makes it cheap in dollars, not in quality. The one case it wins (a reviewer of the parent's own reasoning) does not earn a primitive. 2026-09-17 |

## Shelved designs

- `design/15-server-config.md` — many users, one harness, sandboxed execution.
- `design/40-triggers.md` — schedule files that fire on an event: relay hook, fs watch, `when:` shell check.

## Promoted to a design

- Self-verifying knowledge (`check:` on a concept, a daily sweep, `(stale: …)` hints, run/edit/accept/fix-with-an-agent in both UIs) → `design/41-knowledge-checks.md`, 2026-09-17.
