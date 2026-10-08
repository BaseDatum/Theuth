# 144 — Wave 2 contract (dial · insights · estimate · return-to-notebook · ask · overviews · audio)

Status: frozen for the wave-2 workers (E1, E2, U1, U2) on 2026-10-09 00:05. The design is `docs/design/144-notebooks.md`
(§4.3, §5, §9, §10, §12, §13); Sky's rulings are in `docs/design/144-notebooks.md` §0 and the knowledge entries named in
the briefs. Wave 1 landed on main 2a1b0df6 (as-built: `docs/design/144-as-built-{C,W1,W2,W3,W4,Z}.md`).
Exactly one owner per name below. A worker who needs a change to a frozen name records it under "Deviations" in its
as-built and keeps the frozen name working.

## Sky's additional ruling (2026-10-08 23:56)
"Make sure that when I open a note from a notebook it's easy to return to the notebook." Owner: U1 (§4 below).

## 1. Context dial, estimate, insights — engine (owner E1)

Storage: `crates/agent/migrations/0045_session_context.sql`
`CREATE TABLE session_context (session TEXT NOT NULL, item TEXT NOT NULL, kind TEXT NOT NULL, level TEXT NOT NULL, hlc INTEGER NOT NULL, PRIMARY KEY (session, item));`
(0042 = wave 1 notebook, 0043/0044 = design 146; nothing else may take 0045.)

Types (`crates/document/src/notebook.rs`, ts-rs exported like `ContextLevel`):
```rust
#[serde(rename_all = "lowercase")] pub enum ContextStatus { Available, Truncated, Omitted }
pub struct ContextItem { pub id: String, pub kind: ItemKind, pub title: String,
    pub level: ContextLevel,      // the effective level used at the next turn
    pub pinned: bool,             // true when the user/agent set it (session_context row); false = the default rule
    pub tokens: u32,              // tokens this item contributes at `level` (chars / 4 on the hub)
    pub status: ContextStatus }
pub struct ContextEstimate { pub items: Vec<ContextItem>, pub total_tokens: u32, pub budget_tokens: u32,
    pub omitted: u32, pub empty: bool /* every item off → the "nothing in context" sentence */ }
pub struct InsightInstruction { pub builtin: Option<String>, pub skill: Option<String> } // exactly one set; builtin ∈ key_insights | toc | open_questions
pub struct InsightRun { pub block: String }  // first block id of the appended `###` section
```
RPC methods (`crates/rpc/src/api.rs`, hub-only like the wave-1 notebook methods; TS mirror in U1's `lib/notebooks-rpc.ts`):
- `notebook_context_set(session: SessionId, item: String, kind: ItemKind, level: ContextLevel) -> ()`
- `notebook_context_clear(session: SessionId, item: String) -> ()` (back to the default rule)
- `notebook_context_set_all(session: SessionId, notebook: DocId, kind: Option<ItemKind>, level: ContextLevel) -> ()`
- `notebook_context_estimate(notebook: DocId, session: Option<SessionId>) -> ContextEstimate` (read-only, any engine; `None` = a new thread with the device defaults not yet applied → default rule)
- `insight_run(source: DocId, instruction: InsightInstruction) -> InsightRun`
- `insight_promote(source: DocId, block: String, notebook: Option<DocId>) -> DocId`
- `insight_skills() -> Vec<String>` (the three builtins first, then skills whose front matter has `notebook_insight: true`)
Python runtime tools (agent): `notebook_context_set`, `notebook_context_estimate`, `insight_run`, `insight_promote` — same shapes.
Events: after `after_ingest` or `insight_run` appends to `## Insights`, emit `SourceEvent::Changed { source, status: Completed }` (the UI re-runs the estimate on it).
The `<notebook>` block (`crates/agent/src/notebooks.rs::block`) honours the dial: order full → summary → mention, newest first within a level; `status="truncated"` notice verbatim per §5.3; the allow-list sentence lists only injected ids; every item `off` → the block carries the sentence `nothing in context — the agent sees only the description`.

## 2. Ask, overviews, audio — engine (owner E2)

- `SessionKind::Ask` (`crates/agent/src/lib.rs`): a normal session without a notebook binding; E2 greps the WHOLE workspace for exhaustive `SessionKind` matches and literals before its first gate.
- `AskEvent` (`crates/agent/src/ask.rs`, ts-rs exported; `#[serde(tag = "type", rename_all = "snake_case")]`):
  `Plan { branches: Vec<AskBranch> }` · `Branch { n: u8, hits: Vec<AskHit> }` · `Partial { n: u8, answer: String }` · `Answer { markdown: String }` · `Done { ok: bool, error: Option<String> }`
  `AskBranch { n: u8, notebooks: Vec<DocId>, query: String, instructions: String }` · `AskHit { id: String, block: Option<String>, title: String, score: f32 }`
  Delivered on `subscribe_events` exactly the way `SourceEvent` is (rpc `bridge.rs`, `export.rs`): event name `NotebookAsk { session: SessionId, event: AskEvent }`.
- RPC: `notebook_ask(scope: Vec<DocId>, question: String, save_into: Option<DocId>) -> AskStarted { session: SessionId }` · `notebook_ask_list(limit: Option<u32>) -> Vec<AskSummary>` with `AskSummary { session: SessionId, question: String, scope: Vec<DocId>, created_ms: i64, done: bool }`.
- Overviews: `#[serde(rename_all = "snake_case")] enum OverviewKind { Briefing, Faq, Timeline, MindMap, Audio }` · `#[serde(rename_all = "lowercase")] enum OverviewStatus { Running, Ready, Failed }` ·
  `Overview { kind: OverviewKind, artifact: Option<String> /* call_id */, name: String, version: u32, created_ms: i64, session: SessionId, status: OverviewStatus, note: Option<String>, error: Option<String> }`
  RPC: `notebook_overview_run(notebook: DocId, kind: OverviewKind) -> OverviewRun { session: SessionId }` · `notebook_overviews(notebook: DocId) -> Vec<Overview>` (newest version first per kind).
  An overview's artifact is added to the notebook as `ItemKind::Artifact` when the `show` lands; the audio kind produces TWO artifacts, `overview-script.md` and `overview.mp3` (the `Overview.artifact` is the mp3; the script is listed as `name: "overview-script.md"` with the same version).
- Audio: two speakers in the script always; TTS voice per speaker turn via `SpeechOptions::default().voice(name)`; config `notebooks.audio_voices: Vec<String>` (default `["peter_yearsley"]` + the second embedding's name when E2 ships one); with one voice available both speakers use it and `Overview.note = "one voice available — both hosts read by peter_yearsley"`.

## 3. UI — dial, estimate, insights, chips (owner U1)
Files: `components/sources/{SourceCard,SourcesColumn,SourcePage}.tsx`, `components/notebooks/{ChatColumn,NotesColumn,NotebookContext}.ts(x)`, `components/editor/DocumentPage.tsx`, `components/notes/NotePage.tsx`, `routes/notes_.$id.tsx`, `routes/sources_.$id.tsx`, `lib/notebooks-rpc.ts`, `lib/citations*`, their tests. Hand-written binding stubs for §1 types in `src/rpc/bindings/` (gitignored, never committed).

## 4. Return to the notebook (owner U1) — Sky's ruling
- Every link out of a notebook carries the notebook: `/notes/$id?nb=<notebookId>` (NotesColumn rows, `+ New note`, chips in the notebook chat that open a note, the Save-answer toast's "open"), `/sources/$id?nb=<notebookId>` (already there).
- A note or source page opened with `?nb` shows a back control **at every width** (not `md:hidden`): chevron + the notebook's title ("Back to <title>"), at the left of the `SectionRow`; it navigates to `/notebooks/$nb` with `search: { tab: 'notes' | 'sources' }` so the phone lands on the right column; the desktop restores the three columns.
- Without `?nb`, a note or source that belongs to notebooks shows the same control for its first notebook and a `in <N notebooks> ▾` chip listing all of them (membership from `notebook_items`/`NotebookDetail`), so the way back exists from Library too.
- Pop-out windows keep 143's rule (no band, no back); the main window still shows the notebook.

## 5. UI — ask, overviews, audio (owner U2)
Files: `components/notebooks/{NotebookPage,NotebooksList,NotebookHeaderActions,OverviewsSection,AskDialog,AskView}.tsx`, `routes/ask_.$session.tsx` (new), `components/chat/slash.ts` (`/ask` entry only), `lib/ask-rpc.ts` (new, `Pick<Api>` for §2 methods), the artifact viewer's audio element if missing, their tests. Hand-written binding stubs for §2 types (gitignored, never committed).
`NotebookHeaderActions.tsx` and `OverviewsSection.tsx` exist as placeholders on the base branch; U2 replaces their bodies. `NotesColumn.tsx` (U1) renders `<OverviewsSection notebook={id} />` under the notes list — U1 keeps that one line.

## 6. Shared rules
`~/Develop/theuth-144-briefs/144-WORKING-RULES.md` — mbx for every cargo gate (no CARGO_TARGET_DIR, no clones), build-lock v3.1 etiquette, crate-level gates only, one build at a time, never kill, never push. Migration numbers: 0045 is E1's; nothing else in this wave adds an agent migration. Next free design number is not needed (this is design 144 wave 2).
