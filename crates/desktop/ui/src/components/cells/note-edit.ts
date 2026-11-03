// Owner: U2
// 171 §4.2, §6.3, §7.1: what a note cell computes from its block text — React-free so vitest runs it in node.
// Every edit is ONE `updateBlock` (one ProseMirror transaction, one Y.Doc transaction through y-prosemirror's text
// diff, one undo step); nothing here writes run state into the document.
import type { CellRef } from '@/rpc/bindings/CellRef'
import type { CellRunMode } from '@/rpc/bindings/CellRunMode'
import type { CellSpec } from '@/rpc/bindings/CellSpec'
import { CELL_LANGUAGES } from './CellFence'
import { cellOf } from './grammar'

/** BlockNote's Shell entry: the ```` ```bash ```` input rule and the language menu store `shellscript`, which no engine parses as a cell language. */
const SHELLSCRIPT = 'shellscript'

/** A code block "Make it a cell" applies to (§4.2): a runnable language of v1 — shells and python. */
export const runnableLang = (language: string) => CELL_LANGUAGES.includes(language.trim().toLowerCase()) || language === SHELLSCRIPT

/** ```` ```cells ```` — the first word of the language is `cells` (§3.3). */
export const isDefaultsLang = (language: string) => language.trim().split(/\s+/)[0]?.toLowerCase() === 'cells'

/** `[a-z0-9-]{1,48}` from a heading ("Deploy to ir-prod1" → `deploy-to-ir-prod1`); `step-1` when nothing is left. */
export function slugOf(text: string): string {
  const slug = text
    .normalize('NFKD')
    .replace(/\p{M}/gu, '')
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 48)
    .replace(/-+$/, '')
  return slug || 'step-1'
}

type Doc = { id: string; type: string; content?: unknown; props?: Record<string, unknown>; children?: Doc[] }

/** The text of a block's inline content (styled text runs). */
const plain = (content: unknown) => (Array.isArray(content) ? content.map((c: { text?: string }) => c.text ?? '').join('') : '')

/** Blocks in document order, children after their parent. */
function* walk(blocks: readonly Doc[]): Generator<Doc> {
  for (const b of blocks) {
    yield b
    if (b.children) yield* walk(b.children)
  }
}

/** The text of the last heading before block `id` in document order, else `null`. */
export function nearestHeading(blocks: readonly Doc[], id: string): string | null {
  let last: string | null = null
  for (const b of walk(blocks)) {
    if (b.id === id) return last
    if (b.type === 'heading') last = plain(b.content)
  }
  return last
}

/** Whether `id` is the note's first ```` ```cells ```` block (§3.3: a later one is ignored, with the engine's warning). */
export function firstDefaults(blocks: readonly Doc[], id: string): boolean {
  for (const b of walk(blocks)) if (b.type === 'codeBlock' && isDefaultsLang(String(b.props?.language ?? ''))) return b.id === id
  return true
}

/** What a document code block renders (`editor/CodeBlock.tsx`); `code` = the block's existing path (diagram or code). */
export type BlockRoute = { kind: 'cell'; spec: CellSpec } | { kind: 'guest'; spec: CellSpec } | { kind: 'defaults' } | { kind: 'code' }

/**
 * §4.2 (D14), before the diagram branch: inside an editor that provides `NoteHost`, a runnable language with a `#|`
 * header is a cell (the guest view for a share-link member), the first word `cells` the note's defaults line until
 * the caret enters it; everything else keeps today's path.
 */
export function routeOf(language: string, text: string, host: { guest: unknown } | null, selected: boolean): BlockRoute {
  if (!host) return { kind: 'code' }
  const spec = cellOf(language, text)
  if (spec) return { kind: host.guest ? 'guest' : 'cell', spec }
  return isDefaultsLang(language) && !selected ? { kind: 'defaults' } : { kind: 'code' }
}

/** Make it plain code: the header lines (`spec.header_lines`) removed, the code kept byte for byte. */
export function withoutHeader(text: string, lang: string): string {
  const spec = cellOf(lang, text)
  if (!spec) return text
  return text.replaceAll('\r\n', '\n').split('\n').slice(spec.header_lines).join('\n')
}

/** The `#| python:` value of an interpreter menu pick (§6.3): `.venv`, `uv`, `monty`, else the binary's path. */
export function pythonValue(pick: { kind: string; source: string; path: string | null }): string {
  if (pick.kind === 'monty') return 'monty'
  if (pick.source === '.venv' || pick.source === 'venv') return '.venv'
  if (pick.source.startsWith('uv')) return 'uv'
  return pick.path ?? 'auto'
}

/** Rewrite (or add, after the last header line) the `#| python:` line; every other line is untouched. */
export function withPython(text: string, value: string): string {
  const lines = text.replaceAll('\r\n', '\n').split('\n')
  let header = 0
  while (header < lines.length && (lines[header] as string).startsWith('#|')) header++
  const at = lines.slice(0, header).findIndex((l) => /^#\|\s*python\s*:/.test(l))
  if (at >= 0) lines[at] = `#| python: ${value}`
  else lines.splice(header, 0, `#| python: ${value}`)
  return lines.join('\n')
}

/** §3.4: `Name` when the header names the cell (survives `doc_replace`), else the BlockNote block id. */
export const noteRef = (doc: string, spec: CellSpec, block: string): Extract<CellRef, { type: 'note' }> => ({
  type: 'note',
  doc,
  key: spec.options.name ? { kind: 'name', value: spec.options.name } : { kind: 'block', value: block },
})

/**
 * §7.1 Mod+Enter / the gutter ▶: the mode for a selection `[from, to)` in the block's text (UTF-16 offsets, header
 * included, as ProseMirror counts inside a code block). A selection → `selection` in UTF-8 byte offsets of the code
 * (header excluded, clipped to it); a caret → `line`, 1-based code line. `null` when it touches only the header.
 */
export function modeOf(text: string, spec: CellSpec, from: number, to: number): CellRunMode | null {
  const norm = text.replaceAll('\r\n', '\n')
  const codeStart = norm
    .split('\n')
    .slice(0, spec.header_lines)
    .reduce((n, l) => n + l.length + 1, 0)
  const code = norm.slice(codeStart)
  const a = Math.max(0, Math.min(from, to) - codeStart)
  const b = Math.min(code.length, Math.max(from, to) - codeStart)
  const utf8 = new TextEncoder()
  if (from !== to) return b > a ? { type: 'selection', from: utf8.encode(code.slice(0, a)).length, to: utf8.encode(code.slice(0, b)).length } : null
  if (from < codeStart) return null
  return { type: 'line', line: code.slice(0, a).split('\n').length }
}

export type DiffRow = { op: ' ' | '+' | '-'; line: string }

/**
 * §13.1 "Review diff": a line diff of the owner's last run against the member's text (LCS).
 * ponytail: O(n·m) table — cells are tens of lines; switch to Myers if notes start holding thousand-line cells.
 */
export function lineDiff(before: string, after: string): DiffRow[] {
  const a = before.split('\n')
  const b = after.split('\n')
  const lcs = Array.from({ length: a.length + 1 }, () => new Array<number>(b.length + 1).fill(0))
  for (let i = a.length - 1; i >= 0; i--)
    for (let j = b.length - 1; j >= 0; j--) lcs[i]![j] = a[i] === b[j] ? lcs[i + 1]![j + 1]! + 1 : Math.max(lcs[i + 1]![j]!, lcs[i]![j + 1]!)
  const out: DiffRow[] = []
  let i = 0
  let j = 0
  while (i < a.length && j < b.length) {
    if (a[i] === b[j]) {
      out.push({ op: ' ', line: a[i++] as string })
      j++
    } else if (lcs[i + 1]![j]! >= lcs[i]![j + 1]!) out.push({ op: '-', line: a[i++] as string })
    else out.push({ op: '+', line: b[j++] as string })
  }
  while (i < a.length) out.push({ op: '-', line: a[i++] as string })
  while (j < b.length) out.push({ op: '+', line: b[j++] as string })
  return out
}

/** A member's display name for the engine's `member_edit` pub8 (the share's member list), else the pub8 itself. */
export const memberName = (pub8: string, members: readonly { pub: string; name: string }[] | undefined) => members?.find((m) => m.pub.startsWith(pub8))?.name ?? pub8

/** The slash item "Cell" (§4.2): a third `codeBlock` entry beside "Diagram". */
export const CELL_STARTER: { type: 'codeBlock'; props: { language: string }; content: string } = { type: 'codeBlock', props: { language: 'bash' }, content: '#| name: step-1' }

/** The editor surface the edits need (BlockNote's `BlockNoteEditor`). */
export type BlockEditor = {
  document: readonly Doc[]
  updateBlock(id: string, update: { props?: { language: string }; content?: string }): unknown
}

type CodeBlockLike = { id: string; props: { language: string }; content: unknown }

/** Make it a cell — one transaction: `#| name: <slug of the nearest heading>` as the first line, `shellscript` → `bash`. */
export function makeCell(editor: BlockEditor, block: CodeBlockLike) {
  const update: { props?: { language: string }; content: string } = {
    content: `#| name: ${slugOf(nearestHeading(editor.document, block.id) ?? '')}\n${plain(block.content)}`,
  }
  if (block.props.language === SHELLSCRIPT) update.props = { language: 'bash' }
  editor.updateBlock(block.id, update)
}

/** Make it plain code — one transaction removing the header lines. */
export function makePlain(editor: BlockEditor, block: CodeBlockLike) {
  editor.updateBlock(block.id, { content: withoutHeader(plain(block.content), block.props.language) })
}

/** A mounted note cell and the text on screen (the header's Run all digests it). */
export type MountedCell = { ref: CellRef; text: string }
