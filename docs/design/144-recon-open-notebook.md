# Recon — open-notebook (lfnovo/open-notebook) at /tmp/open-notebook, HEAD a0f6f308 (2026-10-04), MIT

Read-only recon on the shared axes (AXES.md). Every claim cites `path:line` in /tmp/open-notebook (migrations are under
`open_notebook/database/migrations/`). Techniques only; quotes ≤ 5 lines.

## 1. Container model

**Verdict: one container type (notebook), many-to-many via graph edges; a source, a note or a chat session can sit in several notebooks or in none.**

- Schema: `source`, `note`, `notebook` are SCHEMAFULL tables (`open_notebook/database/migrations/1.surrealql:2,34,44`); membership is two RELATION
  tables — `reference` FROM source TO notebook and `artifact` FROM note TO notebook (`1.surrealql:54-60`). Chat sessions join by a third relation
  `refers_to` FROM chat_session TO notebook (`3.surrealql:4-6`), later widened so a session can also refer to a source (`8.surrealql:5`;
  `open_notebook/domain/notebook.py:764-772`).
- Notebook fields are only `name`, `description`, `archived`, `last_viewed_at` (`open_notebook/domain/notebook.py:20-25`); `last_viewed_at` was added
  in migration 18 for both notebook and source (`18.surrealql:4-5`) — recency ordering, not a "pinned/favourite".
- `Notebook.get_sources()` is `select in as source from reference where out=$id` (`notebook.py:34-40`) and `get_notes()` the same over `artifact`,
  omitting `content`/`embedding` unless asked (`notebook.py:52-62`). Membership is created by `Source.add_to_notebook()` → `relate("reference", …)`
  (`notebook.py:532-536`) and `Note.add_to_notebook()` → `relate("artifact", …)` (`notebook.py:739-743`).
- Many-to-many is real and the UI leans on it: the delete preview counts sources "assigned_others" to other notebooks (`notebook.py:184-192`) and
  `Notebook.delete(delete_exclusive_sources=…)` deletes only sources referenced by no other notebook (`notebook.py:215,252-259`). A source is created
  with a `notebooks: [...]` list and related to each (`api/routers/sources.py:506-507`); the source page has a `NotebookAssociations` component
  (`frontend/src/components/sources/NotebookAssociations.tsx`), and search results have "Save to notebooks"
  (`frontend/src/components/search/SaveToNotebooksDialog.tsx`).
- Orphans are allowed: `GET /sources` filters by notebook only when `notebook_id` is given (`sources.py:259,294-301`); search scope `[]` means "whole
  knowledge base" (`notebook.py:781`).
- What a container page shows: three columns — sources, notes, chat (`frontend/src/lib/stores/notebook-columns-store.ts`, see §14) — plus a header
  with inline name/description editing (`frontend/src/app/(dashboard)/notebooks/components/NotebookHeader.tsx`).
- Whole-notebook context for podcasts/LLM is `Notebook.get_context()`: every source as `## Source: <title>` with an "Insights:" bullet list and full
  text, then every note as `## Note: <title>` (`notebook.py:74-138`) — no budget, no ordering beyond insertion.

## 2. Source ingestion

**Verdict: three input kinds (link / upload / text) funnelled through the `content-core` library into one LangGraph (`content_process → save_source →
transform_content*`), run as a background `surreal-commands` job whose status is a field on the source; no content-hash dedupe.**

- Input kinds: `_build_content_state` branches on `source_data.type == "link" | "upload" | "text"` (`api/routers/sources.py:434-472`); multipart form
  carries `notebook_id`, `notebooks` (JSON list), `transformations` (JSON list), `file` (`sources.py:186-198`). A JSON variant exists at `POST
  /sources/json` (`sources.py:708-709`).
- Supported file types are not an allow-list in this repo: a pre-flight `check_file_support()` from content-core rejects unsupported uploads with HTTP
  415 "instead of letting them enqueue a background job that fails and then burns the full retry budget (#975)" (`sources.py:49-72`). YouTube is
  special-cased: `_is_youtube_url` and a tailored error "No transcript or subtitles are available" (`open_notebook/graphs/source.py:85-93`).
- Extraction: `content_core.extract_content` with a `ContentCoreConfig` built from persisted `ContentSettings` — URL engine
  `auto/simple/firecrawl/jina/crawl4ai`, document engine `auto/docling/simple`, `docling_ocr/formulas/vision` flags, preferred YouTube languages
  (`source.py:163-190`). Audio/video goes through the same call: the default STT model's provider/name are passed as `audio_provider`/`audio_model`
  (`source.py:199-208`).
- Save: `save_source` stores `asset(url,file_path)` and `full_text = extraction.content`, keeps a user title unless it is empty or the placeholder
  `"Processing..."` (`source.py:281-290`), then `source.vectorize()` if `embed` (`source.py:295-302`). Notebook edges are made by the API before the
  job for UI responsiveness (`source.py:292-293`, `sources.py:505-507`).
- Transformations fan out with LangGraph `Send` — one `transform_content` branch per selected transformation (`source.py:307-326`), each writing a
  `source_insight` via `source.add_insight(transformation.title, output)` (`source.py:342`). Graph wiring: `source.py:357-366`.
- Async execution: `CommandService.submit_command_job(...)` returns a `command_id`, stored on `source.command` (`sources.py:522-543`;
  `open_notebook/domain/notebook.py:415-417`); `Source.get_status()` asks `surreal_commands.get_command_status` (`notebook.py:437-448`). Status words
  seen by the UI: `new`, `queued`, `running`, `completed`, `failed`, `unknown` (`sources.py:543,867-878,1034`). A sync path
  (`_create_source_sync_path`) is kept as "legacy behavior" with no command_id (`sources.py:566,630`).
- Retry: `POST /sources/{id}/retry` refuses while `running|queued` and otherwise re-submits from `asset.file_path|url` (`sources.py:934-1043`); the
  job itself has a retry config whose `stop_on` lists permanent errors (`commands/source_commands.py:44-61`), with explicit comments on earlier bugs
  where failures were reported as completed (`source_commands.py:172-178`).
- Embedding job: `embed_source` deletes existing `source_embedding` rows (idempotency), detects content type, chunks, embeds in batches, bulk-inserts
  `{source, order, content, embedding}` (`commands/embedding_commands.py:345-395`); retry policy 5 attempts, exponential jitter 1–60 s
  (`embedding_commands.py:31-42`), with a candid note that `stop_on` "can never trigger in practice" (`embedding_commands.py:27-30`).
- Chunking: `RecursiveCharacterTextSplitter` sized in **tokens** (`length_function=token_count`, tiktoken `o200k_base`, word-count fallback) — default
  400 tokens, overlap 15 %, min chunk 5 tokens, env-overridable (`open_notebook/utils/chunking.py:33-57,60-85,88-118,388-395`;
  `open_notebook/utils/token_utils.py:15-41`). Markdown/HTML get header splitters first then secondary chunking (`chunking.py:365-382,471-472`);
  content type is guessed from extension plus a scored heuristic with a 0.8 override threshold (`chunking.py:119,322-353`).
- Dedupe: nothing — the only "duplicate" in the router is a comment about notebook edges (`sources.py:505`). No hash of file or URL is stored
  (`1.surrealql:4-11`).
- Failure handling: permanent vs transient classification in every command (`source_commands.py:85-104,160-182`); the frontend polls status
  (`frontend/src/lib/hooks/use-sources.ts` — see §14).

## 3. Source representation

**Verdict: a source is one row with `full_text` + `asset{file_path,url}` + `title` + `topics`, plus two child tables — ordered embedded chunks
(`source_embedding`) and typed AI "insights" (`source_insight`); addressed by SurrealDB record id `source:<key>`.**

- Row fields: `asset` (flexible object), `title`, `topics[]`, `full_text`, `created/updated` (`open_notebook/database/migrations/1.surrealql:4-14`);
  `last_viewed_at` (`18.surrealql:5`); `command` link to the processing job (`8.surrealql:12`). Domain mirror:
  `open_notebook/domain/notebook.py:406-417`.
- Chunks: `source_embedding {source, order, content, embedding}` (`1.surrealql:16-20`); count per source via `get_embedded_chunks()`
  (`notebook.py:502-512`).
- Insights: `source_insight {source, insight_type, content, embedding}` (`1.surrealql:22-26`), embedding later made optional (`10.surrealql:8`),
  timestamps added in 19 (`19.surrealql:7-8`). `insight_type` is the transformation's *title* (`open_notebook/graphs/source.py:342`). An insight can
  be promoted to a note: `SourceInsight.save_as_note()` titles it `"{insight_type} from source {title}"` (`notebook.py:394-397`).
- Cascade: a DB event deletes chunks and insights when the source row disappears (`1.surrealql:29-32`) and `Source.delete()` also removes the file and
  both child tables explicitly (`notebook.py:650-680`).
- Context shapes: `Source.get_context("short")` = `{id,title,insights}`; `"long"` adds `full_text` (`notebook.py:480-500`). The file is downloadable
  at `GET /sources/{id}/download` with a path-traversal guard ("Access to file denied", `api/routers/sources.py:716-735,824`).
- Topics exist in the schema but nothing in the pipeline writes them (grep `topics` in `graphs/` finds only the field; `notebook.py:412`).
- Status/progress derive from the linked command (`notebook.py:437-477`), exposed at `GET /sources/{id}/status` (`sources.py:843-902`).

## 4. Notes model

**Verdict: a note is one markdown string with an optional title and a `note_type ∈ {human, ai}`; no blocks, no versions, no links other than notebook
membership; AI notes get an LLM-generated title.**

- Schema: `note {title, summary, content, embedding}` (`1.surrealql:34-42`), `note_type` added in `2.surrealql:1`; domain `Note(title, note_type:
  Literal["human","ai"], content)` with a non-empty-content validator (`open_notebook/domain/notebook.py:691-702`). `summary` is in the schema but not
  in the domain model (`notebook.py:691-695`).
- Save embeds asynchronously: `Note.save()` submits an `embed_note` command, fire-and-forget (`notebook.py:704-732`); the command retries 5×
  (`commands/embedding_commands.py:227-228`).
- Title rule: when an `ai` note arrives without a title the API runs the generic prompt graph with "provide a Title for this content, with max 15
  words" (`api/routers/notes.py:59-73`). Human notes keep whatever title was typed (`notes.py:60`).
- Linking: only `artifact` edges to notebooks (`notebook.py:739-743`); no note→source or note→note relation exists in the migrations
  (`1.surrealql:54-60`, `3.surrealql:4-6`). Provenance of an AI note is only its title text (`notebook.py:397`).
- Editor: `NoteEditorDialog` is a react-hook-form + zod dialog defaulting `note_type: 'human'`
  (`frontend/src/app/(dashboard)/notebooks/components/NoteEditorDialog.tsx:3-7,117`) wrapping `MarkdownEditor` (`NoteEditorDialog.tsx:12,174`), which
  is `@uiw/react-md-editor` loaded dynamically with `remark-math` + `rehype-katex` + `rehype-sanitize`, `preview: 'live'|'edit'|'preview'`
  (`frontend/src/components/ui/markdown-editor.tsx:5-14,51,60-85`). Plain markdown textarea — not a block editor.
- Versioning/history: none — `PUT /notes/{id}` overwrites `title/content/note_type` in place (`notes.py:146-171`); no history table in any migration
  (`open_notebook/database/migrations/` listing: tables are source, source_embedding, source_insight, note, notebook, chat_session, transformation,
  episode_profile, speaker_profile, episode, credential, …).
- Context shape: `Note.get_context("short")` returns the first 100 characters of content; `"long"` the whole note (`notebook.py:745-755`).
- List: `GET /notes?notebook_id=` (`notes.py:17-38`); the notes column renders cards
  (`frontend/src/app/(dashboard)/notebooks/components/NotesColumn.tsx`, §14).

## 5. Context control

**Verdict: three per-item states for sources (off / insights-only / full) and two for notes (off / full), chosen per notebook page and sent with every
turn as a string-matched `context_config`; the server renders the context and returns a tiktoken estimate; no hard budget for notebook chat, only a
model switch above 105 k tokens; a 50 k budget exists only in source chat.**

- Protocol: the frontend maps each selection to the strings `'insights'`, `'full content'`, `'not in'`
  (`frontend/src/lib/hooks/use-notebook-chat.ts:133-160`); the server matches `"not in" in status`, `"insights" in status`, `"full content" in status`
  and the module docstring says "That protocol is shared with the frontend — do not change it here"
  (`open_notebook/utils/context_builder.py:12-14,296-324`).
- Types: `ContextMode = 'off' | 'insights' | 'full'`, notes exclude `'insights'` (`frontend/src/lib/types/notebook-context.ts:2-3`). Bulk actions in
  the sources column header: `include | insights | full | exclude`; `include` = "insights if the source has any, else full" and is the implicit
  default on first load (`frontend/src/lib/utils/source-context.ts:3-26`; menu at
  `frontend/src/app/(dashboard)/notebooks/components/SourcesColumn.tsx:176`). The rule is unit-tested
  (`frontend/src/lib/utils/source-context.test.ts:17-27`).
- Selections are not persisted: they live in page state (`frontend/src/app/(dashboard)/notebooks/[id]/page.tsx:71-110`); docs confirm "Selections are
  not saved… reset when you reload" (`docs/2-CORE-CONCEPTS/ai-context-rag.md` § "Context Levels in Notebook Chat").
- Assembly: `POST /api/chat/context` calls `build_notebook_context(notebook, context_config)` and returns `{context, token_count, char_count}` with
  `token_count()` (tiktoken o200k_base) (`api/routers/chat.py:395-412`; `open_notebook/utils/token_utils.py:15-41`). With no config it falls back to
  "all sources and notes with short context" (`context_builder.py:325-340`). What "short" means: `{id,title,insights}` for a source and the first 100
  chars of a note (`open_notebook/domain/notebook.py:480-500,745-755`).
- The context string is then sent by the client as `request.context` on each `/chat/execute` and stored into graph state (`chat.py:341`), where the
  system prompt interpolates it under `# CONTEXT` (`prompts/chat/system.jinja:18-24`). The UI shows "N sources, M notes in context · tokens · chars"
  above the composer (`frontend/src/app/(dashboard)/notebooks/components/ChatColumn.tsx:43-66`).
- Sizing: no truncation for notebook chat. The only budget logic is `provision_langchain_model`: `if tokens > 105_000` use the `large_context` default
  model, else explicit `model_id`, else the role default (`open_notebook/ai/provision.py:19-34`; also stated in `open_notebook/AGENTS.md:24`). Output
  capped at `max_tokens=8192` (`open_notebook/graphs/chat.py:46`).
- Source chat has a real budget: `build_source_context(source_id, max_tokens=50000)` (`open_notebook/graphs/source_chat.py:83-86`) reserves 20 % for
  insights (`SOURCE_INSIGHT_BUDGET_RATIO = 0.2`, `context_builder.py:37,428`), truncates the text at a token boundary and appends "[Source content
  truncated to fit the context token budget.]" (`context_builder.py:34-36,102-131`), reporting `source_text_status ∈ {available, truncated,
  omitted_budget}` (`context_builder.py:471-477`).
- Insights-only is only offered when the source has ≥ 1 insight (`source-context.ts:23-26`; docs § "Insights only needs insights").

## 6. Chat

**Verdict: chat sessions are first-class rows related to a notebook (or a source), each with its own LangGraph thread checkpointed in SQLite; one
node, no tools, no streaming for notebook chat; model override per session or per request; "Save to note" creates an `ai` note with an LLM-made
title.**

- Session row: `chat_session {title, model_override}` SCHEMALESS (`open_notebook/database/migrations/3.surrealql:2`;
  `open_notebook/domain/notebook.py:758-762`), linked by `refers_to` to a notebook or source (`notebook.py:764-772`). `GET
  /chat/sessions?notebook_id=` lists them; `POST/PUT/DELETE /chat/sessions` manage title and `model_override`
  (`api/routers/chat.py:97-98,142-143,240-253,287-288`).
- Graph: `ThreadState{messages, notebook, context, context_config, model_override}`, one node `agent` → END, compiled with
  `SqliteSaver(LANGGRAPH_CHECKPOINT_FILE)` (`open_notebook/graphs/chat.py:22-27,95-105`). Message history therefore lives in a **SQLite checkpoint
  file, not SurrealDB** — a second store (§15).
- Turn: `/chat/execute` reads the thread state, sets `context`, `notebook`, `model_override`, appends a `HumanMessage` with an explicit id "so a
  failed turn can remove it from the checkpoint", and runs the sync graph in a thread (`chat.py:308-369`). Model precedence: request override >
  session override > role default, and the 105 k switch above everything (`chat.py:324-329`; `open_notebook/ai/provision.py:23-34`).
  Empty-after-thinking responses raise instead of being checkpointed (`graphs/chat.py:76-84`).
- Streaming: notebook chat is explicitly "synchronous, no streaming" (`frontend/src/lib/hooks/use-notebook-chat.ts:175`); the whole message list comes
  back (`chat.py:375-377`). Source chat and Ask stream SSE (`api/routers/source_chat.py:460-467`; `api/routers/search.py:177-185`).
- Tools: `open_notebook/graphs/tools.py` defines a single `@tool get_current_timestamp` (13 lines, `tools.py:7-8`) and the chat node does not bind it
  — the system prompt's "search tool" language (`prompts/chat/system.jinja:40,47`) is vestigial. There is **no agent loop** anywhere in the chat path.
- Save answer as note: `MessageActions` → `createNote({content, note_type:'ai', notebook_id})` with the comment "Title will be auto-generated by the
  API for AI notes" (`frontend/src/components/sources/MessageActions.tsx:21-33`; `api/routers/notes.py:59-73`). Ask results use
  `SaveToNotebooksDialog` with `note_type: 'ai'` into several notebooks (`frontend/src/components/search/SaveToNotebooksDialog.tsx:58`).
- Thinking blocks are stripped with `clean_thinking_content` before storing (`graphs/chat.py:77`); math rendering instructions are in the system
  prompt (`system.jinja:26-28`).

## 7. Ask / research across containers

**Verdict: Ask is a three-stage LangGraph — a strategy model writes up to five search terms with instructions, each runs a top-10 vector search in
parallel (`Send`) and a per-search answer model writes a partial answer with ids, then a final model synthesises; single-turn, vector-only, scoped by
notebook ids or the whole base; three model slots.**

- State and schema: `Strategy{reasoning, searches: List[Search{term, instructions}]}` "up to five searches" (`open_notebook/graphs/ask.py:40-52`);
  `ThreadState{question, strategy, answers (operator.add), final_answer, notebook_ids}` (`ask.py:55-62`).
- Stage 1 `call_model_with_messages`: `PydanticOutputParser(Strategy)` over `prompts/ask/entry.jinja` (`ask.py:65-75`); blank terms are dropped and an
  all-blank strategy raises a user-facing error (`ask.py:92-104`). The prompt gives a worked JSON example and "Do not include ```json``` in the
  response" (`prompts/ask/entry.jinja:16-38`).
- Fan-out: `trigger_queries` returns one `Send("provide_answer", …)` per search (`ask.py:112-125`); `provide_answer` calls `vector_search(term, 10,
  True, True, notebook_ids=…)` — notes and sources both on — and returns `{"answers": []}` on no hits (`ask.py:134-142`). A commented-out text-search
  branch shows hybrid was considered and dropped (`ask.py:131-133`).
- Per-search answer prompt receives `question`, `term`, `instructions`, `results` and the explicit allow-list "You have been given the following
  content ids to work from: {{ids}}" (`prompts/ask/query_process.jinja:5-27,48-51`). The model role used is `"tools"` (`ask.py:149-153`).
- Stage 3 `write_final_answer` renders `prompts/ask/final_answer.jinja` with `strategy` and `answers` and the rule "Only use the ids of the documents
  that you can see on the answers you received" (`ask.py:169-176`; `final_answer.jinja:13-23,39`).
- Budget: `ASK_MAX_TOKENS = 8192` for all three stages, raised from 2000 because reasoning models had no room after thinking (`ask.py:23-27`).
- Endpoint: `POST /search/ask` streams SSE events `strategy` → `answer`* → `final_answer` → `complete` (`api/routers/search.py:77-124,140-185`);
  `/search/ask/simple` returns only the final answer (`search.py:202-259`). Each of the three models must exist or 400 (`search.py:149-166`).
- Scope: `resolve_notebook_scope()` validates ids and `[]` means the whole knowledge base; the DB function takes `$notebook_ids`
  (`open_notebook/domain/notebook.py:775-790,905`).
- Limits admitted by the docs: "Ask needs an embedding model", "Ask only sees embedded content", "Ask is single-turn. There are no follow-ups"
  (`docs/2-CORE-CONCEPTS/ai-context-rag.md` § "Ask: Retrieval Over Your Knowledge Base"). No reranking, no dedupe across the five searches beyond
  `GROUP BY id` inside one search (`3.surrealql:64-72`).

## 8. Citations

**Verdict: citations are bare record ids in square brackets — `[source:xyz]`, `[note:xyz]`, `[source_insight:xyz]` (with `insight:` tolerated as an
alias) — pointing at whole items, never chunks or passages; the frontend regex-rewrites them into numbered links `[1](#ref-type-id)` with a References
list, clicks open a modal, and a deleted target renders a shared "unavailable" state.**

- Grammar asked of the model: "adding the id of the specific document in between brackets like this: [document_id]", with the worked example
  `[note:iuiodadalknda]. … [insight:adadadadadadad].` and "Always use the complete ID exactly as it is provided, including its type prefix"
  (`prompts/chat/system.jinja:30-47`; identical blocks in `prompts/ask/query_process.jinja:29-46` and `prompts/ask/final_answer.jinja:33-41`). The
  per-search answer prompt pins the allowed set: "You have been given the following content ids to work from: {{ids}}" (`query_process.jinja:48-51`).
- What ids the model actually sees: the vector search returns `id` of the **chunk** (`source_embedding:…`) or insight or note, plus `parent_id`
  (`open_notebook/database/migrations/3.surrealql:13-18,28-33,44-49`); for notebook chat the context renders `**Insight ID:** {id}` lines
  (`open_notebook/utils/context_builder.py:61-70`) and source dicts with `id` (`open_notebook/domain/notebook.py:493-500`). So a chat answer can only
  cite a source, a note or an insight; Ask can emit a `source_embedding:` id that the frontend grammar does **not** recognise (see next bullet) — an
  unverified edge, noted in §20.
- Parsing: `/(source_insight|insight|note|source):([a-zA-Z0-9_]+)/g`, "`insight:` is accepted as an alias for `source_insight` — some models emit the
  short form" (`frontend/src/lib/utils/source-references.tsx:46-59,179-197`). Nested/bold/mixed forms like `[source:x, note:y, source_insight:z]` are
  handled by a 50-char look-back for brackets (`source-references.tsx:41,172,219`).
- Rendering: `convertReferencesToCompactMarkdown` replaces each reference with `[n](#ref-{type}-{id})` and appends a "References:" section
  (`source-references.tsx:331-342,372-405`); a custom link component intercepts `href.startsWith('#ref-')`, picks an icon (`Lightbulb` for insights)
  and calls `onReferenceClick(type, id)` (`source-references.tsx:275-317,440-475`). In the chat panel that opens the matching modal
  (`frontend/src/components/sources/ChatPanel.tsx:81-97,387-391`).
- Design tokens exist for "evidence classes (citation chips)" (`frontend/src/app/dev/design/page.tsx:236`; `frontend/src/app/globals.css:107`).
- Moved/deleted targets: `ContentUnavailable` is "used by the source, note and insight dialogs so that dangling references (e.g. citations in old chat
  messages pointing at deleted items) render the exact same explanation everywhere" (`frontend/src/components/common/ContentUnavailable.tsx:17-24`).
  Nothing re-resolves or strikes out the citation in the message itself.
- Granularity is documented honestly: "Citations point to the whole item, not to a page or passage" (`docs/2-CORE-CONCEPTS/ai-context-rag.md:115`;
  `docs/3-USER-GUIDE/citations.md:9`), "a citation can be missing or point to the wrong item. Treat citations as a pointer to check, not as proof"
  (`citations.md:40`), "There is no export or share feature for citations" (`citations.md:49`). Transformations produce no citations
  (`citations.md:24`).

## 9. Search

**Verdict: two separate modes exposed as a radio — BM25 full-text (SurrealDB `SEARCH ANALYZER`, English snowball stemmer) or cosine vector over
chunk/insight/note embeddings with a 0.2 floor — no hybrid fusion, no reranking, no ANN index; everything embedded is searchable (chunks, insights,
notes); chats are not; rebuild is a background job over existing or all items.**

- Full-text: analyzer `my_analyzer TOKENIZERS blank,class,camel,punct FILTERS snowball(english), lowercase` and BM25 indexes with highlights on
  `source.title`, `source.full_text`, `source_embedding.content`, `source_insight.content`, `note.content`, `note.title`
  (`open_notebook/database/migrations/1.surrealql:65-72`). `fn::text_search` unions six sub-selects and groups by id with `math::max(relevance)`
  (`3.surrealql:79-145`); highlights are wrapped in backticks (`3.surrealql:84`). English-only stemming is admitted: "Matching in other languages is
  less forgiving" (`docs/2-CORE-CONCEPTS/ai-context-rag.md` § "Text search (keywords)").
- Vector: `fn::vector_search($query, $match_count, $sources, $show_notes, $min_similarity)` runs brute-force `vector::similarity::cosine` over
  `source_embedding`, `source_insight`, `note` and unions them (`3.surrealql:10-73`, later versions add `$notebook_ids` — `9.surrealql:12-15`,
  `open_notebook/domain/notebook.py:905`). No `DEFINE INDEX … MTREE|HNSW` exists in any migration (grep over `migrations/` for MTREE/HNSW: none) —
  scans are linear. Default `minimum_score=0.2`, `results` = requested limit (`notebook.py:888-895`).
- Mode selection: `POST /search` with `type == "vector"` requires an embedding model, else text (`api/routers/search.py:25-51`). A text-search runtime
  error "position overflow" (SurrealDB highlight bug, issue #648) silently falls back to vector search (`notebook.py:858-870`), and a double failure
  is surfaced rather than returned as `[]` (`notebook.py:871-875`).
- What is embedded and how: sources → one vector per chunk (`commands/embedding_commands.py:385-394`); notes and insights → **one vector per record**,
  mean-pooled across chunks when longer than `CHUNK_SIZE` (`embedding_commands.py:107-130`; `open_notebook/utils/embedding.py:206-250`). Query
  embedding uses the same function, so a long query is chunked and pooled too (`notebook.py:899-902`). Batches of 50 texts, env
  `OPEN_NOTEBOOK_EMBEDDING_BATCH_SIZE` (`embedding.py:24-45`).
- Chunk parameters: 400 tokens / 15 % overlap / min 5 tokens, env-overridable, tiktoken-measured
  (`open_notebook/utils/chunking.py:33-57,60-85,88-118`).
- Rebuild: `POST /embedding/rebuild` with `mode: "existing" | "all"` and `include_sources/notes/insights` flags, returning a command id; `GET
  /rebuild/{command_id}/status` (`api/routers/embedding_rebuild.py:19-30,128-129`); the coordinator command `rebuild_embeddings` has `retry=None` and
  submits per-item jobs (`embedding_commands.py:530,603,634-651`). Embedding-model change therefore requires an explicit rebuild — nothing stores
  which model produced a vector (`1.surrealql:16-26`: no model/dimension field).
- Scope: both functions accept `notebook_ids`; `[]` = whole base; ids are validated first (`notebook.py:775-790`). Frontend scope picker:
  `frontend/src/components/search/NotebookScopeSelector.tsx`.
- Not searchable: chat messages (they live in the LangGraph SQLite checkpoint, `open_notebook/graphs/chat.py:95-99`) and podcast transcripts.

## 10. Transformations / reusable actions

**Verdict: "transformations" are named prompt rows (`name, title, description, prompt, apply_default, model_id`) seeded by migration with six
defaults; run on a source they create a `source_insight` typed by the transformation's title; one can be flagged `apply_default` to run on every new
source; they also run ad hoc on arbitrary text; execution is a background command.**

- Schema and seed: table `transformation` with `apply_default DEFAULT False` (`open_notebook/database/migrations/5.surrealql:8-16`); six seeded rows —
  "Analyze Paper", "Key Insights", "Dense Summary", "Reflections", "Table of Contents", "Simple Summary" (`5.surrealql:21,61,89,104,129,143`); only
  "Dense Summary" has `apply_default: True` (`5.surrealql:101`). The prompts are long "IDENTITY and PURPOSE / STEPS / OUTPUT INSTRUCTIONS" patterns
  (`5.surrealql:24-57`, Fabric-style).
- Domain: `Transformation(name, title, description, prompt, apply_default, model_id)` plus a singleton `DefaultPrompts.transformation_instructions`
  record (`open_notebook/domain/transformation.py:9-30`), editable at `GET/PUT /transformations/default-prompt`
  (`api/routers/transformations.py:139-161`) — a global preamble prepended to every transformation (`open_notebook/graphs/transformation.py:47-52`).
- Execution: graph renders `prompts/transformation/execute.jinja` = `{{ instructions }}` + math-format note + `# INPUT` (`execute.jinja:1-7`),
  `provision_langchain_model(..., "transformation", max_tokens=8192)` (`graphs/transformation.py:55-67`); a `finish_reason == "length"` is detected
  (`transformation.py:77`). Role fallback: transformation default → chat default (`open_notebook/ai/models.py:342-345`).
- Where output lands: on a source it becomes `source_insight{insight_type = transformation.title, content}` via the async `create_insight` command,
  which then submits `embed_insight` (`open_notebook/graphs/source.py:333-342`; `open_notebook/domain/notebook.py:584-632`;
  `commands/embedding_commands.py:434-442`). It is **not** a note; the user can promote it with `save_as_note()` (`notebook.py:394-397`) and the docs
  say transformations "don't produce citations" (`docs/3-USER-GUIDE/citations.md:24`).
- Entry points: at source creation (`transformations: [...]` in the form, `api/routers/sources.py:186-198,671-677`), later via `POST
  /sources/{id}/insights` which returns 202 with a command id (`sources.py:1104-1148`) and `run_transformation` command
  (`commands/source_commands.py:191-228`), or on raw text via `POST /transformations/execute` (`api/routers/transformations.py:93-126`).
- No transformation runs on a note; the only AI-on-note path is the title generator (`api/routers/notes.py:59-73`). User-defined transformations are
  plain CRUD (`transformations.py:37-59,207-257`).
- A generic one-shot prompt graph (`prompts/pattern/generic.jinja` = `{{ prompt }}` + optional format instructions, `generic.jinja:1-5`) backs title
  generation.

## 11. Audio & voice

**Verdict: podcast generation is delegated to the external `podcast-creator` library — episode profile (outline model, transcript model, briefing,
segments) + speaker profile (TTS model, 1–4 speakers with voice_id/backstory/personality) → outline → transcript → TTS → ffmpeg combine — as one
background command; the reverse direction is only STT for audio/video *sources* via content-core; there is no voice-note capture or dictation in the
UI.**

- Tables: `episode_profile {speaker_config, outline_provider/model, transcript_provider/model, default_briefing, num_segments DEFAULT 5}`,
  `speaker_profile {tts_provider, tts_model, speakers[].{name, voice_id, backstory, personality}}`, `episode {briefing, episode_profile,
  speaker_profile, transcript, outline, command, content, audio_file}` (`open_notebook/database/migrations/7.surrealql:1-42`).
- Pipeline: `generate_podcast` command (`retry max_attempts 1`) resolves outline/transcript/TTS models from the profiles, configures `podcast_creator`
  with the speaker profiles keyed by name, calls `create_podcast`, and stores `transcript`, `outline`, and `audio_file` relative to `PODCASTS_FOLDER`
  (`commands/podcast_commands.py:20-24,120-198,358-406`). In-band `"ERROR: ..."` audio paths from the library are turned into a failed job after
  persisting the transcript (`podcast_commands.py:383-413`).
- Prompts: `prompts/podcast/outline.jinja` takes `briefing`, `context` (string or list of `<content_piece>`), `speakers` and a language instruction
  with a fixed JSON contract `segments[{name, description, size ∈ short|medium|long}]` (`outline.jinja:1-30`); `transcript.jinja` is the second step.
  Output is capped at 8192 tokens per step, surfaced to users in help text (`frontend/src/lib/locales/en-US/index.ts:666`).
- Context fed to a podcast: the user picks sources/notes per notebook in `GeneratePodcastDialog`
  (`frontend/src/components/podcasts/GeneratePodcastDialog.tsx:65-104`); whole-notebook text comes from `Notebook.get_context()`
  (`open_notebook/domain/notebook.py:74-138`).
- Failure explanations are hand-written for the common library errors (invalid speaker name, bad voice_id, truncated JSON)
  (`podcast_commands.py:46-90`).
- Reverse direction: the default STT model is passed to content-core as `audio_provider/audio_model` so audio/video uploads become text
  (`open_notebook/graphs/source.py:199-208`); roles `speech_to_text`/`text_to_speech` are created through esperanto's `AIFactory`
  (`open_notebook/ai/models.py:271-278`). The UI explains "Not set — transcription unavailable until configured" (`en-US/index.ts:882`). No
  `MediaRecorder`/`getUserMedia` anywhere in `frontend/src` (grep: none) — no voice notes.

## 12. Models & providers

**Verdict: one singleton record holds seven role defaults — chat, transformation, large_context, text_to_speech, speech_to_text, embedding, tools —
resolved at call time by `provision_langchain_model(content, model_id, default_type)` with the order: >105 k tokens → large_context; explicit id →
that model; else the role default (with transformation/tools/large_context falling back to chat); providers are abstracted by `esperanto` and keyed by
stored credentials.**

- Defaults record `open_notebook:default_models` with the seven fields (`open_notebook/ai/models.py:165-174`), always fetched fresh
  (`models.py:177-182`). Role → field mapping and fallbacks in `get_default_model()` (`models.py:337-356`); a missing default logs "go to Manage →
  Models" and returns `None` (`models.py:357-362`).
- Selection at call time: `provision_langchain_model` (`open_notebook/ai/provision.py:10-34`) — used by chat (`"chat"`,
  `open_notebook/graphs/chat.py:45-46`), ask (`"tools"` for per-search answers, `graphs/ask.py:149-153`), transformations (`"transformation"`,
  `graphs/transformation.py:63-67`). Rule restated for contributors: "never instantiate provider clients directly. It auto-upgrades to
  large_context_model above 105,000 tokens (hard-coded threshold)" (`open_notebook/AGENTS.md:24`).
- Per-object overrides: chat sessions carry `model_override` (`open_notebook/domain/notebook.py:760-762`); transformations carry `model_id`
  (`open_notebook/domain/transformation.py:17`); Ask takes three explicit model ids (`api/routers/search.py:149-166`); episode/speaker profiles carry
  their own (`7.surrealql:5-8,19-20`).
- Providers: `esperanto` `AIFactory.create_*` for language, embedding, STT, TTS (`models.py:4,271-278`); credentials are rows in a `credential` table
  (`12.surrealql:6-26`) converted with `to_esperanto_config()` (`models.py:228`). STT is also handed to content-core (`graphs/source.py:199-208`).
- Capabilities endpoint and model routers exist for the UI (`api/routers/{models,providers,credentials,capabilities}.py` — listing in `api/routers/`),
  not read in depth.
- Output caps: 8192 everywhere (chat `graphs/chat.py:46,69`; ask `graphs/ask.py:27`; transformation `graphs/transformation.py:67`; podcast help text
  `frontend/src/lib/locales/en-US/index.ts:666`).

## 13. API / agent surface

**Verdict: a complete FastAPI REST surface (114 routes across 22 routers) behind an optional single shared password sent as `Authorization: Bearer
<password>`; the UI is a pure client of it ("the API is complete by construction"); agent access is an external PyPI MCP server (`open-notebook-mcp`)
that wraps the same REST — nothing agent-shaped lives in this repo, and there is no tool-calling surface inside the product.**

- Routers: `api/routers/` holds `auth, capabilities, chat, commands, config, credentials, embedding, embedding_rebuild, episode_profiles, insights,
  languages, models, notebooks, notes, podcasts, providers, search, settings, source_chat, sources, speaker_profiles, transformations` (directory
  listing). Route count: `grep -rhoE '@router\.(get|post|put|delete|patch|head)\(' api/routers | wc -l` = **114**.
- Key resources covered above: `GET/POST /sources`, `/sources/{id}/{status,retry,download,insights}`
  (`api/routers/sources.py:257,639,708,755,809,824,843,905,934,1055,1075,1104`); `/notes` CRUD (`api/routers/notes.py:17,55,121,146,189`);
  `/chat/sessions`, `/chat/execute`, `/chat/context` (`api/routers/chat.py:97,142,184,240,287,308,395`); `/search`, `/search/ask` (SSE),
  `/search/ask/simple` (`api/routers/search.py:25,140,202`); `/transformations…` (`api/routers/transformations.py:37,58,93,139,160,185,207,256`);
  `/embedding/rebuild` (`api/routers/embedding_rebuild.py:19,128`).
- Async contract: anything slow returns a `command_id`; `GET /commands/{id}` for status (`api/routers/commands.py`; design rationale in
  `docs/7-DEVELOPMENT/decisions/ADR-004-background-workers.md:13` — "Submission is fire-and-forget (returns a job id immediately), status is
  observable (`/commands/{id}`), failures are explicit (permanent vs. retriable)").
- Auth: `PasswordAuthMiddleware` — "Auth is fully disabled (no hardcoded default password) if OPEN_NOTEBOOK_PASSWORD is not set", Docker-secret file
  supported, excluded paths `/`, `/health`, `/docs`, `/openapi.json`, `/redoc` (`api/auth.py:12-30`); expected header "Bearer {password}", 401 with
  `WWW-Authenticate: Bearer` otherwise (`api/auth.py:49-60`); `GET /auth/status` tells the UI whether a password is needed
  (`api/routers/auth.py:13-20`). One shared secret, no users, no tokens, no scopes.
- MCP: docs describe `uvx open-notebook-mcp` with `OPEN_NOTEBOOK_URL` for Claude Desktop/VS Code — "Access your notebooks", "Search your research
  content", "Create and manage chat sessions", "Generate notes" (`docs/5-CONFIGURATION/mcp-integration.md:7-36`). No `mcp` string in any `.py` under
  the repo (shell grep: none) — the server is a separate package. ADR-003 explains the enabling choice: "The API is complete by construction —
  anything the UI does, an integration can do (this later enabled the MCP direction, #878)"
  (`docs/7-DEVELOPMENT/decisions/ADR-003-streamlit-to-nextjs.md:23`).
- In-product agent surface: none — the chat graph has one node and binds no tools (`open_notebook/graphs/chat.py:102-105`;
  `open_notebook/graphs/tools.py:7-8` is the only `@tool`).
- OpenAPI is served by FastAPI (`/docs`, `/openapi.json` excluded from auth, `api/auth.py:28-29`); contributors are told the UI must never call the DB
  directly (`open_notebook/AGENTS.md`, the API-boundary rule, and `docs/7-DEVELOPMENT/decisions/ADR-003-streamlit-to-nextjs.md`).

## 14. UI

**Verdict: notebook page = three columns (Sources ⅓ | Notes ⅓ | Chat flex) with the two side columns collapsible to a 12-px rail and the state
persisted in localStorage; on < lg it becomes a three-tab switcher; the source page is a Content / Insights / Details tab set with an inline YouTube
embed and a "Chat with source" button; the note editor is a dialog around a markdown textarea with live preview; shadcn/Radix + Tailwind; empty states
are a shared component; keyboard support is thin (Cmd/Ctrl-Enter to send, arrow keys in the sources list).**

- Layout: desktop `hidden lg:flex … flex-row`, Sources column `sourcesCollapsed ? 'w-12 flex-shrink-0' : 'flex-none basis-1/3'`, same for Notes, Chat
  `flex-1 min-w-0` (`frontend/src/app/(dashboard)/notebooks/[id]/page.tsx:223-265`). Collapse state is a zustand `persist` store named
  `notebook-columns-storage` (`frontend/src/lib/stores/notebook-columns-store.ts:1-27`).
- Mobile: `lg:hidden` `Tabs` with `grid-cols-3` triggers `sources | notes | chat` (`page.tsx:165-181`) and a note "Detect desktop to avoid
  double-mounting ChatColumn" (`page.tsx:56`).
- Sources column: cards with a per-source context-mode control (`frontend/src/app/(dashboard)/notebooks/components/SourcesColumn.tsx:236`) and a bulk
  context menu in the header (`SourcesColumn.tsx:176`); status badges derive from the command status (`api/routers/sources.py:325-369`); empty state
  `sources.noSourcesYet` (`SourcesColumn.tsx:218-220`).
- Notes column: cards with a context toggle (`NotesColumn.tsx:167-170`), empty state `notebooks.noNotesYet` (`NotesColumn.tsx:133-135`); the editor is
  `NoteEditorDialog` → `MarkdownEditor` (`NoteEditorDialog.tsx:12,174`), i.e. `@uiw/react-md-editor` in `live` mode with KaTeX and sanitised preview
  (`frontend/src/components/ui/markdown-editor.tsx:13-14,51,60-85`).
- Chat column: context summary line (sources/notes in context, tokens, chars) above the composer (`ChatColumn.tsx:43-66`); Cmd/Ctrl-Enter sends in the
  source chat panel (`frontend/src/components/sources/ChatPanel.tsx:261-263`, tested in `ChatPanel.test.tsx:63,86`); session rename/create via
  Enter/Escape (`SessionManager.tsx:133,182-183`).
- Source page: tabs `content | insights | details` (`frontend/src/components/sources/SourceDetailContent.tsx:522-526`), YouTube `<iframe
  src=https://www.youtube.com/embed/…>` when the URL matches (`SourceDetailContent.tsx:384-385,547-548`), a transformation `Select` + run button in
  the Insights tab (`SourceDetailContent.tsx:601-619`) and an insights list (`SourceDetailContent.tsx:649`); "Chat with source" only in the modal
  (`SourceDetailContent.tsx:466`). The sources index page has ArrowUp/ArrowDown/Enter navigation
  (`frontend/src/app/(dashboard)/sources/page.tsx:107-128`).
- Component stack: Radix primitives (accordion, dialog, dropdown, popover, tabs, tooltip…), `cmdk`, `sonner`, `lucide-react`, `react-hook-form` +
  `zod`, `@tanstack/react-query`, `zustand`, `react-markdown` + `remark-gfm` + `rehype-katex` (`frontend/package.json` dependencies: 46 runtime, 17
  dev). i18n via `i18next` with 15 locales (bn-IN, ca-ES, de-DE, en-US, es-ES, fr-FR, it-IT, ja-JP, pl-PL, pt-BR, ru-RU, tr-TR, zh-CN, zh-TW;
  `frontend/src/lib/locales/` listing).
- Design system: "Quiet Green" tokens (content-type hues, evidence/citation classes, context-state colours, 4–6 px radii, near-flat shadows) landed in
  1.15.0 (`CHANGELOG.md:31`; `frontend/src/app/dev/design/page.tsx:220-236`; `frontend/src/app/globals.css:107`).
- No global command palette or keyboard shortcut layer was found (grep for `useHotkeys|CommandDialog` in `frontend/src`: none beyond the `cmdk`
  dependency).

## 15. Storage & sync

**Verdict: SurrealDB (schemaful tables + relations + BM25 indexes + DB functions) as the system of record, migrated at API startup from numbered
`.surrealql` files with `_down` pairs; chat history in a separate LangGraph SQLite checkpoint file; uploads and podcast audio on the local `./data`
folder; no export/import, no sync, no offline mode — self-hosted single instance.**

- Migrations: 25 up files + 25 down files under `open_notebook/database/migrations/` (directory listing; `ls *.surrealql | grep -v _down | wc -l` =
  25; 1,976 lines of SurrealQL in total). Policy: "Migrations run automatically at API startup, in sequence, tracked in `_sbl_migrations`" and "One
  migration per PR that needs one; numbers allocated in merge order; never consolidate after a migration has touched main"
  (`docs/7-DEVELOPMENT/decisions/ADR-006-migration-granularity.md:13,18`).
- Schema shape: SCHEMAFULL tables for `source`, `note`, `notebook`, `source_embedding`, `source_insight`, `transformation`, `episode_profile`,
  `speaker_profile`, `episode`, `credential` (`1.surrealql:2,16,22,34,44`; `5.surrealql:8`; `7.surrealql:1,16,31`; `12.surrealql:6`); SCHEMALESS
  `chat_session` and `podcast_config` (`3.surrealql:2`; `1.surrealql:62`); RELATION tables `reference`, `artifact`, `refers_to` (`1.surrealql:54-60`;
  `3.surrealql:4-6`); search functions `fn::text_search`/`fn::vector_search` (`3.surrealql:10,79`); DB-side cascade event on source delete
  (`1.surrealql:29-32`).
- Second store: chat threads are LangGraph checkpoints in `./data/sqlite-db/checkpoints.sqlite` (`open_notebook/config.py:5-10`;
  `open_notebook/graphs/chat.py:95-99`) — chat text is therefore outside SurrealDB's search indexes and backup story.
- Files: uploads under `./data/uploads`, podcasts under `./data/podcasts` (`config.py:13-20`); source rows keep `asset.file_path`/`asset.url`
  (`open_notebook/domain/notebook.py:322-324`); podcast audio paths are stored relative to `PODCASTS_FOLDER` (#1030)
  (`commands/podcast_commands.py:395-402`); downloads guarded against path traversal (`api/routers/sources.py:716-735`).
- Job queue: `surreal-commands` rides on the same SurrealDB — "zero infrastructure for self-hosters"; a Celery move "is under evaluation"
  (`ADR-004-background-workers.md:15`); "A worker process is required for anything async to actually run … forgetting it is a silent-queue failure
  mode" (`ADR-004:25`).
- Export/import: none in the API routers (no `export`/`import` route among the 114; grep of `docs/3-USER-GUIDE/working-with-notes.md` for
  export/import: none). The docs rewrite explicitly removed "Invented features … (tags, export/share, batch transformations, page-number citations and
  others)" (`CHANGELOG.md:13`). Citations: "There is no export or share feature" (`docs/3-USER-GUIDE/citations.md:49`).
- Sync/offline: none — single server, browser client, no service worker (no `sw.js`/`workbox` in `frontend/src`). Migration safety is documented as
  "back-up-before-migrating warning" (`CHANGELOG.md:14`).
- Credentials for providers are stored encrypted in the DB with PBKDF2 (`docs/7-DEVELOPMENT/decisions/ADR-009-pbkdf2-credential-encryption.md`;
  `CHANGELOG.md:14`).

## 16. Sharing & privacy

**Verdict: single-user by product decision (PDR-001); one optional shared password; no roles, no share links, no per-item permissions; privacy =
self-hosting plus per-source "not in context" so a cloud provider never receives it — but Ask ignores that toggle.**

- Policy: "Open Notebook remains a single-user product for now — but … new features must not gratuitously preclude multi-user"
  (`docs/7-DEVELOPMENT/decisions/PDR-001-single-user-first.md:13`); multi-user umbrella is issue #712 (`PDR-001:5`).
- Auth: one `OPEN_NOTEBOOK_PASSWORD` for the whole API, disabled when unset (`api/auth.py:12-24`); the install docs put the "network-exposure warning
  (bind ports to 127.0.0.1 or set a password)" before the first `docker compose up` (`CHANGELOG.md:12`).
- No sharing: no share/publish/link routes among the 114 (router listing, §13); "There is no export or share feature for citations"
  (`docs/3-USER-GUIDE/citations.md:49`).
- What leaves the machine: only what the user sends to a configured provider — "A source set to *Not included in chat* is not sent by Chat" but "A
  source excluded from Chat can still reach the model through Ask if it is embedded and Ask is not limited to other notebooks"
  (`docs/2-CORE-CONCEPTS/ai-context-rag.md:35,37`). Content extraction can also call external engines (firecrawl/jina) when configured
  (`open_notebook/graphs/source.py:169-180`); YouTube proxy/cookies options exist (`CHANGELOG.md:29`).
- Provider secrets: encrypted at rest with PBKDF2-derived keys (`ADR-009-pbkdf2-credential-encryption.md`), with Docker-secret file support for the
  password (`api/auth.py:17`).
- No telemetry hooks found (grep `telemetry|analytics` in `SECURITY.md`: none; not exhaustively verified across the frontend).

## 17. Numbers

Computed read-only in `/tmp/open-notebook` (shell `find/wc/grep -c`, 2026-10-08):

| Measure | Value | How |
|---|---|---|
| HEAD | `a0f6f3081ae0…` dated 2026-10-04 | `git log -1 --date=short` (shallow clone: `git log --oneline | wc -l` = 1, so no cadence from git) |
| Latest release | **1.15.0 — 2026-10-04** (`CHANGELOG.md:26`), plus an `[Unreleased]` docs-rewrite block (`CHANGELOG.md:8-24`) | |
| Release cadence | 25 tagged versions since 1.2.4 (2025-12-14): 1.4.0 → 1.5.x → 1.6.x (Jan 2026), 1.7.x (Feb), 1.8.x (Mar–Apr), 1.9/1.10 (Jun), 1.11–1.14 (Jul), 1.15 (Oct) (`CHANGELOG.md:26,76,111,142,186,256,290,322,…,629`) | grep `^## \[` |
| Python files / LOC | 81 files / 21,105 lines (`open_notebook/`, `api/`, `commands/`) | `find … -name '*.py'` |
| TS/TSX files / LOC | 252 files / 43,702 lines (`frontend/src`) | |
| REST routes | 114 across 22 router files | `grep -rhoE '@router\.(get|…)\('` |
| Python tests | 69 `test_*.py` files, 781 `def test_` functions (`tests/`) | |
| Frontend tests | 35 `*.test.ts(x)` files, 153 `it(`/`test(` cases | |
| Migrations | 25 up + 25 down, 1,976 SurrealQL lines | `ls migrations` |
| Prompts | 9 `.jinja` files (`prompts/`) | |
| Docs | 80 markdown pages; 15 decision records (13 ADR + 2 PDR; `docs/7-DEVELOPMENT/decisions/`) | |
| Python deps | 19 top-level in `pyproject.toml` `dependencies` (FastAPI, LangChain/LangGraph 1.x pins, tiktoken, `langgraph-checkpoint-sqlite`, six `langchain-*` provider shims "runtime requirements of esperanto's dynamic `to_langchain()`") (`pyproject.toml` dependencies block) | |
| Frontend deps | 46 runtime + 17 dev (`frontend/package.json`) | |
| TODO/FIXME/HACK | **1** real (`open_notebook/graphs/tools.py:6` "todo: turn this into a system prompt variable"); the naive case-insensitive count of 22 is Spanish/Portuguese "todos" in locale files | `grep -rniE`, then read |
| Providers | "24 providers" per the docs rewrite (`CHANGELOG.md:12`) | |
| Stars/forks | **not verifiable offline** — README only carries a live shields.io badge (`README.md:5,317-318`), no number in the tree; the brief forbade network access | |
| Licence | MIT (`LICENSE`; brief) | |

Notes: `TSLOC` includes tests and locale files (the `en-US` locale alone is > 900 lines, `frontend/src/lib/locales/en-US/index.ts:882`). Python LOC
excludes `tests/` and `scripts/`.

## 18. Techniques worth borrowing

Ranked for a product that already has an agent with tools and block-level note editing. Each: the technique, the cite, the theuth seam (named by
function).

1. **Three-state per-item context dial (off / insights-only / full) with a live token+char readout before the turn.** The user picks what the model
  sees per source and per note, defaults are "insights if the source has any, else full", and the composer shows "N sources, M notes · tokens · chars"
  computed server-side with tiktoken (`frontend/src/lib/utils/source-context.ts:3-26`; `api/routers/chat.py:395-412`; `ChatColumn.tsx:43-66`). Seam:
  **note page chat panel / attachment picker** — a per-attachment dial (`off | summary | full`) plus a pre-turn token estimate, and the same dial on
  meeting notes and knowledge hints. Cheap, no model call, and directly addresses "what did the model actually see".
2. **Typed "insight" records as a first-class derived artefact, distinct from notes, promotable to a note.** `source_insight{source, insight_type,
  content, embedding}` where `insight_type` is the transformation title; separately embedded; `save_as_note()` promotes with a provenance title
  (`1.surrealql:22-26`; `graphs/source.py:342`; `domain/notebook.py:394-397`). Seam: **knowledge harvest / document summaries** — store summaries, key
  points, TOC as typed children of the artifact or note (searchable on their own), and let "insights-only" context use them. theuth's harvest already
  writes knowledge; the borrow is the *typed child + promote* shape and the UI tab (`SourceDetailContent.tsx:522-526`).
3. **Named prompt actions ("transformations") with `apply_default` and a global preamble.** Rows `{name, title, description, prompt, apply_default,
  model_id}`, six seeded, one on-by-default for every new source, a user-editable instruction block prepended to all of them (`5.surrealql:8-16,101`;
  `domain/transformation.py:9-30`; `graphs/transformation.py:47-52`). Seam: **doc_* tools / skills** — a "run action on this note/attachment"
  affordance in the note page and the attachment card; in theuth these are skills, so the borrow is the *default-on-ingest* flag and the result
  landing as an insight (item 2), not a new prompt store.
4. **Citation grammar = full record id in brackets, with an explicit allow-list in the prompt and alias-tolerant parsing.** `[note:abc]`,
  `[source_insight:abc]`, "You have been given the following content ids to work from: {{ids}}", regex accepting `insight:` as alias, rewritten to
  `[n](#ref-type-id)` + References list, click → modal, deleted target → shared "unavailable" state (`prompts/ask/query_process.jinja:31-51`;
  `source-references.tsx:46-59,331-405`; `ContentUnavailable.tsx:17-24`). Seam: **search() / knowledge hints / meeting summary** — theuth already
  injects ids for knowledge, sessions, artifacts and notes; the borrow is (a) the allow-list sentence per turn, (b) one client-side rewrite to
  numbered chips with a References footer, (c) a uniform dangling-reference card. Block-level anchors (`note:id#block`) would exceed what they do —
  they admit "whole item, not a page or passage" (`docs/3-USER-GUIDE/citations.md:9`).
5. **Ask = strategy → parallel searches with per-search *instructions* → per-search answer → synthesis, streamed as typed SSE events.**
  `Strategy{reasoning, searches[{term, instructions}]}` ≤ 5, `Send` fan-out, `ids` allow-list per branch, `strategy | answer | final_answer |
  complete` events (`graphs/ask.py:40-62,112-161`; `api/routers/search.py:100-124`). Seam: **search() across notes/knowledge/sessions/meetings** —
  theuth's agent can do this with tools, so the borrow is the *shape*: show the plan (strategy) and the partial answers as they arrive, and pin each
  branch's citations to its own result set.
6. **Token-measured chunking with content-type-aware splitters and a min-chunk floor.** 400 tokens / 15 % overlap / drop < 5 tokens, markdown/HTML
  header splitters first, extension + heuristic type detection with a 0.8 override (`utils/chunking.py:33-118,322-353,365-395,471-480`). Seam:
  **knowledge/notes/attachment indexing** — if theuth adds vector search over notes or artifacts, these are sane defaults; the min-floor exists
  because providers "return null vector elements" for tiny inputs.
7. **Source-chat budget with a reserved insight share and an honest truncation notice.** 50 k budget, 20 % reserved for insights then returned to
  text, "[Source content truncated to fit the context token budget.]", status `available | truncated | omitted_budget` (`graphs/source_chat.py:83-86`;
  `utils/context_builder.py:34-37,102-131,428-477`). Seam: **attachment injection into a turn** — tell the model (and the user) what was cut.
8. **Large-context auto-upgrade at a fixed token threshold.** `tokens > 105_000 → large_context` default model regardless of the explicit pick
  (`ai/provision.py:19-34`). Seam: **model routing per turn** — theuth has effort/model per session; a "big-context fallback model" role is a one-line
  rule.
9. **Explicit "status word" contract for background ingest with a retry endpoint and permanent-vs-transient classification.** `new | queued | running
  | completed | failed | unknown`, `POST /sources/{id}/retry` refusing while running, pre-flight 415 for unsupported files so a job never burns
  retries (`api/routers/sources.py:49-72,843-902,934-1043`; `commands/source_commands.py:44-61`). Seam: **attachment/artifact ingest and knowledge
  harvest jobs** — show the state on the card and offer retry.
10. **Many-to-many container membership with "exclusive sources" delete semantics.** `reference`/`artifact` relation tables, delete preview counting
  `assigned_others`, `delete_exclusive_sources` flag (`1.surrealql:54-60`; `domain/notebook.py:165-192,215-259`). Seam: **notes list / projects** — if
  theuth groups notes and artifacts into projects, borrow the "also in N other projects" preview before delete.
11. **Insight-only defaults chosen by what exists.** `includedMode(insightsCount) = insightsCount > 0 ? 'insights' : 'full'`
  (`source-context.ts:23-26`) — a tiny rule that keeps cost down without asking the user.
12. **Empty-response guard before checkpointing.** Raise instead of persisting a blank AI turn so it is never replayed as context
  (`graphs/chat.py:76-84`). Seam: **session history** — theuth likely has this; confirm.

## 19. Do-not-borrow

- **The one-node, tool-less chat graph and the vestigial "search tool" prompt text** (`graphs/chat.py:102-105`; `prompts/chat/system.jinja:40,47`). It
  exists because they have no agent; theuth's agent already searches with tools. Borrowing the "send the whole selected context every turn" model
  would regress theuth.
- **Vector-only Ask with brute-force cosine and no fusion/reranking** (`3.surrealql:10-73`; `graphs/ask.py:131-140`). No ANN index (no MTREE/HNSW in
  migrations), no hybrid; the text-search branch was commented out. theuth's pre-turn assembly survey already favours lexical id match + one-hop
  expansion over pure vectors.
- **A second store for chat history (LangGraph SQLite checkpoints)** (`graphs/chat.py:95-99`; `config.py:10`). Splits backup, search and retention;
  theuth keeps sessions in one sqlite — keep it that way.
- **Context selections that live only in page state** (`page.tsx:71-110`; docs "Selections are not saved"). A notebook-like theuth should persist the
  dial per note/thread.
- **Plain-markdown textarea editor (`@uiw/react-md-editor`) and whole-string `PUT /notes/{id}`** (`markdown-editor.tsx:13-14`;
  `api/routers/notes.py:146-171`). theuth already has block-level `doc_*` editing with stable ids; their model has no blocks, no history, no anchors.
- **Whole-item citations** (`docs/3-USER-GUIDE/citations.md:9`). theuth can cite `path#Lx-Ly` and block ids; do not drop below that.
- **String-matched context protocol ("not in" / "insights" / "full content" substrings)** (`utils/context_builder.py:12-14,296-318`). Use an enum.
- **Prompt-embedded Fabric-style mega-prompts as seed rows** (`5.surrealql:24-57`). theuth's skills already carry instructions in files with front
  matter.
- **`stop_on` retry lists that "can never trigger in practice"** (`commands/embedding_commands.py:27-30`) — a pattern to avoid, not copy.
- **Podcast generation via an external black-box library with hand-written error explainers** (`commands/podcast_commands.py:20-24,46-90`). theuth has
  its own TTS/STT stack; if an audio overview is ever wanted, keep outline → script → TTS in-house where the prompts are
  (`prompts/podcast/outline.jinja`).
- **Single shared password auth** (`api/auth.py:12-24`). theuth's RPC/bearer token model is already stronger.

## 20. Weaknesses and open complaints

Admitted or visible in the tree (no network; GitHub issues are cited only where the code/docs cite them):

- **No agent, no tools, no follow-ups in Ask.** "Ask is single-turn. There are no follow-ups" (`docs/2-CORE-CONCEPTS/ai-context-rag.md:82`); the chat
  graph binds no tools (`graphs/chat.py:102-105`); the only `@tool` is a timestamp with a `# todo` (`graphs/tools.py:6-8`). VISION lists "Agents
  operating Open Notebook — role inversion via MCP" as a *horizon* cluster (#878, #693, #973) (`VISION.md:71`).
- **Citations are whole-item and unvalidated.** "a citation can be missing or point to the wrong item. Treat citations as a pointer to check, not as
  proof" (`docs/3-USER-GUIDE/citations.md:40`); ADR-008 defers "validated citations" and an "evidence bundle" to discussion #1315 "deliberated until
  October 2026" (`ADR-008-notebook-scoped-search.md:9,13`). Ask's vector results carry `source_embedding:` chunk ids (`3.surrealql:13-18`) that the
  frontend grammar does not parse (`source-references.tsx:53`) — likely rendered as plain text; **not verified at runtime**.
- **Privacy toggle is partial.** "A source excluded from Chat can still reach the model through Ask if it is embedded" (`ai-context-rag.md:37`).
- **Context selections reset on reload** (`ai-context-rag.md:26`; `page.tsx:71-110`).
- **No streaming in notebook chat; sync LangGraph in a thread** (`use-notebook-chat.ts:175`; `api/routers/chat.py:352-358`).
- **Search: English-only stemmer; SurrealDB highlight "position overflow" bug worked around by silently switching to vector search (#648)**
  (`1.surrealql:65`; `domain/notebook.py:858-870`; docs "Matching in other languages is less forgiving"). No ANN index → linear cosine scans (no
  MTREE/HNSW in `migrations/`). Embedding-model change needs a full rebuild and nothing records which model produced a vector (`1.surrealql:16-26`;
  `api/routers/embedding_rebuild.py:19-30`).
- **Two stores and a required worker.** Chat history in SQLite checkpoints (`config.py:10`); "A worker process is required for anything async to
  actually run … forgetting it is a silent-queue failure mode" (`ADR-004-background-workers.md:25`); queue move to Celery "under evaluation" with a
  SurrealDB v3 migration as "one coordinated breaking change" (`VISION.md:68`).
- **Retry config that cannot fire**: "`stop_on` below can never trigger in practice … to be revisited in a dedicated error-handling PR"
  (`commands/embedding_commands.py:27-30`); earlier bug where failed extraction was reported `completed` (`commands/source_commands.py:172-178`).
- **Schema drift.** `note.summary` and `source.topics` exist in the schema but nothing writes them (`1.surrealql:10,37`;
  `domain/notebook.py:412,691-695`); `podcast_config` SCHEMALESS leftover (`1.surrealql:62`); a Portuguese comment "entender o analyzer"
  (`1.surrealql:64`).
- **Docs admitted to have described an imagined product** until the rewrite: "Invented features are gone (tags, export/share, batch transformations,
  page-number citations and others). About 6,200 lines became about 1,450" (`CHANGELOG.md:13`); `API_CLIENT_TIMEOUT` "was never read"
  (`CHANGELOG.md:18`).
- **Single-user, one password, no export/share** (`PDR-001-single-user-first.md:13`; `api/auth.py:12-24`; `citations.md:49`).
- **Notes are thin**: one markdown string, no history, no links to sources, AI-note provenance only in the title (`notes.py:146-171`;
  `domain/notebook.py:397,739-743`).
- **Output caps and model quirks pushed to users**: 8192-token caps everywhere with help text telling users to lower them for weak models
  (`graphs/ask.py:23-27`; `en-US/index.ts:666`); podcast failures explained by string-matching library errors (`podcast_commands.py:46-90`).
- **Release gap**: 1.14.0 (2026-07-20) → 1.15.0 (2026-10-04), 2.5 months, with the big 1.15 item being a docs rewrite and a design-token system
  (`CHANGELOG.md:26-31,76`).

## Self-check

Re-opened with `read_file(ranges=…)` after the report was drafted (26 cites, all in `/tmp/open-notebook`):

- `open_notebook/database/migrations/1.surrealql:54-60` — ok (reference FROM source TO notebook; artifact FROM note TO notebook)
- `open_notebook/database/migrations/3.surrealql:4-6` — ok (refers_to FROM chat_session TO notebook)
- `open_notebook/domain/notebook.py:694` — ok (note_type Literal human/ai)
- `open_notebook/domain/notebook.py:745-755` — ok (Note.get_context short = content[:100])
- `open_notebook/graphs/source.py:357-366` — ok (graph wiring content_process -> save_source -> transform_content)
- `open_notebook/utils/chunking.py:57` — ok (return 400 default chunk size)
- `open_notebook/utils/chunking.py:85` — ok (overlap int(chunk_size*0.15))
- `open_notebook/utils/context_builder.py:12-14` — ok (string-matching protocol shared with frontend)
- `open_notebook/utils/context_builder.py:37` — ok (SOURCE_INSIGHT_BUDGET_RATIO = 0.2)
- `open_notebook/ai/provision.py:23-28` — ok (tokens > 105_000 -> large_context)
- `open_notebook/graphs/source_chat.py:85` — ok (max_tokens=50000)
- `open_notebook/graphs/ask.py:134-140` — ok (vector_search(term, 10, True, True, notebook_ids))
- `open_notebook/graphs/ask.py:27` — ok (ASK_MAX_TOKENS = 8192)
- `prompts/chat/system.jinja:32` — ok (citation grammar [document_id])
- `prompts/ask/query_process.jinja:50-51` — ok (ids allow-list)
- `frontend/src/lib/utils/source-references.tsx:53` — ok (regex (source_insight|insight|note|source))
- `frontend/src/components/common/ContentUnavailable.tsx:20-22` — ok (dangling references explanation)
- `frontend/src/lib/hooks/use-notebook-chat.ts:175` — ok (synchronous, no streaming)
- `api/auth.py:15-16` — ok (auth disabled if OPEN_NOTEBOOK_PASSWORD unset)
- `api/routers/notes.py:64` — ok (title prompt max 15 words)
- `open_notebook/database/migrations/5.surrealql:101` — ok (apply_default: True (Dense Summary))
- `commands/embedding_commands.py:27-30` — ok (stop_on can never trigger)
- `docs/7-DEVELOPMENT/decisions/ADR-004-background-workers.md:25` — ok (worker required, silent-queue failure)
- `docs/3-USER-GUIDE/citations.md:40` — ok (citation can be missing / pointer to check)
- `CHANGELOG.md:26` — ok (1.15.0 - 2026-10-04)
- `open_notebook/graphs/tools.py:6-8` — ok (todo + get_current_timestamp)

Counts in §17 were re-run once in a single read-only shell (`find`/`wc`/`grep -c`/`ls`); the TODO count was corrected after reading the matches
(locale false positives).

Not verified (no network, no execution): GitHub stars/forks/issue counts; runtime behaviour of `source_embedding:` ids in Ask answers (§8, §20); the
external `open-notebook-mcp` package's tool list; `podcast-creator`'s internals; content-core's exact supported file list (only the 1.15.0 CHANGELOG
line and the 415 pre-flight were read).

Method: skeleton written first, then axes filled in four passes (1–4, 5–8, 9–12, 13–20), saving after each; all `path:line` cites were taken from
`read_file` line numbers in this session. Nothing under `/tmp/open-notebook` was modified; the only file written is this report.

