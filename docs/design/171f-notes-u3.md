# 171F-U3 — no transcript element closes by itself (B13)

Base `0daf1005` (C skeleton). UI only, `crates/desktop/ui/src/`.

## Mechanism
- `components/chat/disclosure.tsx` (new): `DisclosureScope` context (`Message` provides `entry.id`; `FenceScope` adds
  `fence|<offset of the fence body in the reply>`), `useScopedDisclosure(part, initial)` = C's `useDisclosure` under
  `disclosureKey(scope, part)`, falling back to the mount's `useId` outside any scope (artifact viewer, boards, attention
  screens: as local as the `useState` it replaced). `Details` = Radix `Collapsible` with store state, replacing every
  `<details>`. `useDisclosed(pick)` reads a primitive derived from the open map (live state as server snapshot).
  `FenceText` carries the reply markdown for `FenceScope` in its own context, so a text delta re-renders only the scopes.
- Why a scope context instead of props: the call sites in `Message.tsx` `body()` (~:27-78) are U2's; the cards pick their
  key up from context, so no line there changed. `useStampToggle` (also used by U2's `MessageUser`) is keyed the same way.
- Tool cards: `toolKey(callId)` (`PlainTool`, `ScrapeCard`, `RosterCard`; `ArtifactCard` errors under it). `ToolRunCard`
  has its own key `run|<first callId>` whose initial value is "any member's `toolKey` is open": a lone expanded tool that
  the next call folds into a run opens the run, and the card inside shows expanded (T1). Opening a run does not expand
  its first card. `useToolRuns.open/toggle` became dead and were removed from `lib/tool-runs.ts` (not a K1 fix; a clean
  cutover of the run state onto C's `disclosed`).
- Prompt cards key by prompt id (`ask|<id>`), not by entry.
- T4: `MessageAssistant` decides streamdown `mode` once per mount (`useState` initial from `open`): mounted streaming, it
  stays in streaming mode after the stream closes (same output); mounted settled (reload), static.
- T5: `JsonRenderFence` boundary key is `open`/`closed`, no longer `code`.
- T6a: `Card` always renders the same `div` wrapper; `context` only adds the padding class and the remove button.
- T6b: `signature()` of an origin-less peer is `p\n<text.trimEnd()>` (live `from` is `job j4` / `session:<id>` /
  `reminder r3`, stored `from` is the formatted label / the child's name).

## Keys (`disclosed`)
| site | key |
|---|---|
| PlainTool, ScrapeCard, RosterCard | `tool|<callId>` |
| ArtifactCard Show errors | `tool|<callId>|errors` |
| ToolRunCard | `run|<first callId>` (initial: any member `tool|…` open) |
| AskCard relay → full | `ask|<ask id>` |
| MessagePeer / Provenance | `<entry>|peer`, `<entry>|provenance` |
| EngineNotice, Tombstone | `<entry>|notice`, `<entry>|tombstone` (initial `reveal`) |
| AdvisorNoteCard | `<entry>|advisory`, `<entry>|advisory-more` |
| MessageVision, MessageCompacted | `<entry>|vision`, `<entry>|summary` |
| Desk/Resume/Lost/ParentView, Section | `<entry>|desk` … `<entry>|view`, `<entry>|section|<title>` |
| MeetingCard OracleRow | `<entry>|oracle|<batch_id>` |
| SkillCard sheet, HtmlPage errors | `<entry>|skill|<name>`, `<entry>|page-errors|<callId>` |
| Stamp (coarse tap) | `<entry>|stamp` |
| AttachChip / CitationLink / DoubleChip popovers | `<entry>|attach|<kind>|<id>`, `<entry>|cite|<href>`, `<entry>|double|<ids>` |
| Attachments lightbox | `<entry>|lightbox|<item key>` (one flag per item) |
| FenceFallback / OpenUI Show source | `<entry>|fence|<offset>|source` |
| surface Tree / Accordion / Tabs / Image | `<fence>|tree|/<i>:<label>…`, `<fence>|accordion|<i>|<title>`, `<fence>|tabs|<labels>|<i>`, `<fence>|picture|<src>` |
| Excerpt Show all, DiagramFrame fullscreen | `<scope>|excerpt|<path>|<start>`, `<scope>|fullscreen|<kind>` |

## Decisions (ambiguities, simpler option taken)
- json-render passes no element key to catalog components, so surface elements key by their own content (title, label
  path, src) under the fence scope; while an element's own text is still streaming its key moves (toggling a half-written
  title is not a case worth more machinery), and two identical elements in one fence share a flag.
- `DiagramFrame` fullscreen keys by `kind` (content hash would change mid-stream): two same-kind diagrams in one fence
  open together (`ponytail:` comment in place). Pan/zoom `view` is not a disclosure and stays local.
- A fence body not found verbatim in the reply (fence indented inside a list item) scopes by its first 80 characters.
- `Details` mounts its body only while open (Radix default, as `MessageReasoning`): closed `<details>` bodies used to sit
  hidden in the DOM; find-in-page no longer reaches them.
- `ChildLostCard`: the child link sits inside the trigger button now; its click stops propagation so it does not toggle.
- Ratchet scope: `components/chat/**` + `components/cells/**`, but header/composer/menu chrome files (`CHROME` in the
  test: AdvisorPill, AttachPicker, CostPill, HeaderMenu, HeldStrip, ModelCombobox, ModelMenu, OrphansBanner) are skipped:
  not transcript, never remounted by a stream, and a menu's open state in a global store would reopen it after
  navigation. `useState` hits count only boolean state with a disclosure-shaped name.
- T6b pairs by body: verified for `<job_result>` and `<reminder>` (`supervisor.rs` `job_result`/`reminder` wrap the
  delivered `text` verbatim between newlines; `classify` strips them and `trimEnd`s). The `<subagent_result>` body is
  assumed to be the delivered text the same way (its writer was not traced); if not, that card still loses its id.

## Allowlist (Z clears each entry once the owner's branch is merged; the test also fails on a stale entry)
| entry | owner |
|---|---|
| `cells/Cell.tsx:useState:open`, `cells/Cell.tsx:useState:raw` | U1b |
| `cells/KernelStrip.tsx:useState:open` | U1b |
| `cells/NoteCell.tsx:useState:open`, `cells/NoteCell.tsx:useState:review` | U1b |
| `cells/vendor/nteract/outputs/ansi-output.tsx:useState:expanded` | U1b |
| `cells/RunbookBar.tsx:useState:why` | U1a |

U2's files (`MessageUser.tsx`, `ResultCard.tsx`, `LongText.tsx`, `SentResults.tsx`) have no hit on this base.
Not matched by the ratchet (non-boolean state) but a disclosure in U1b's file: `json-output.tsx` `expandedPaths` (a
`Set` of opened JSON paths); U1b converts it.

## Not covered by the contract table (static render cannot show them)
Radix portals (SkillCard sheet, Attachments lightbox, DiagramFrame fullscreen, the popovers' content — AttachChip is
covered through its trigger's `data-state`), HtmlPage errors (needs iframe console messages), Stamp (coarse pointer
only), DoubleChip / CitationLink / AskCard / Provenance / ArtifactCard (need prediction, citation, packet or artifact
stores). They use the same hook. No live app run (manual check optional per brief).

## Verification
- Tests fail on the old code: with `reduce.ts`, `MessagePeer.tsx` and `registry.tsx` checked out from `0daf1005`,
  `disclosure.test.ts` fails 7 (ratchet, peer, accordion, tree, tab, picture, settled reload) and passes after.
- Throwaway smoke (deleted): streamdown streaming mode with `isAnimating=false` renders the same markup as `static` for a
  heading/list/table/json-render reply except `\n` text nodes between blocks (no visual difference), so a reply mounted
  while streaming and kept in streaming mode looks settled; a reply mounted settled is static. The same smoke found that
  streamdown hands an open fence its body plus a trailing newline: `FenceScope` trims before the offset search (the
  permanent "through streamdown" test covers it, mid-stream and settled, both modes).
