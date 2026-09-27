// Design 71 §2.2: the board's input is the document's block tree, read headlessly — `yXmlFragmentToBlocks` over a
// `BlockNoteEditor` that is never mounted (only its ProseMirror schema is used). Pure helpers here so the block
// mapping is testable without React: which code block is an `Excerpt`, a diagram, mermaid or plain code; the
// anchors the document carries; the frontmatter `root`.
import { BlockNoteEditor } from '@blocknote/core'
import { yXmlFragmentToBlocks } from '@blocknote/core/yjs'
import { type DocBlock, type DocumentSchema, FRAGMENT, links, schema } from '@theuth/documents/schema'
import { get, parseHeader } from '@theuth/documents/yaml'
import { useEffect, useState } from 'react'
import type * as Y from 'yjs'
import { type Anchor, parseAnchor } from '@/components/chat/surface/anchor'
import type { ExcerptProps } from '@/components/chat/surface/Excerpt'
import { excerptLines, parseSpec } from '@/components/chat/surface/parse'
import type { SequenceData } from '@/components/chat/surface/Sequence'
import type { GraphData } from '@/lib/graph-layout'
import type { EngineProvider } from '@/lib/doc/provider'

export type { DocBlock }

/** Inline content as BlockNote types it: styled text runs and links (the two inline specs the schema keeps). */
export type Inline = Extract<DocBlock, { type: 'paragraph' }>['content'][number]

type Reader = BlockNoteEditor<DocumentSchema['blockSchema'], DocumentSchema['inlineContentSchema'], DocumentSchema['styleSchema']>
let reader: Reader | null = null

/** The document's top-level blocks (typed, with ids); an empty fragment is an empty board. */
export function readBlocks(fragment: Y.XmlFragment): DocBlock[] {
  if (fragment.length === 0) return []
  reader ??= BlockNoteEditor.create({ schema, links })
  return yXmlFragmentToBlocks(reader, fragment)
}

/** Live blocks of one provider: re-read on every Yjs change (an insert is one new element on the reader, M7). */
export function useBlocks(provider: EngineProvider): DocBlock[] {
  const fragment = provider.doc.getXmlFragment(FRAGMENT)
  const [blocks, setBlocks] = useState(() => readBlocks(fragment))
  useEffect(() => {
    const update = () => setBlocks(readBlocks(fragment))
    update()
    fragment.observeDeep(update)
    return () => fragment.unobserveDeep(update)
  }, [fragment])
  return blocks
}

/** The plain text of a block's inline content — the claim line a prose anchor pins with. */
export function plainText(content: DocBlock['content']): string {
  if (!Array.isArray(content)) return ''
  return (content as Inline[]).map((c) => (c.type === 'text' ? c.text : c.type === 'link' ? c.content.map((t) => t.text).join('') : '')).join('')
}

/** `root:` from the leading frontmatter block (71 §2.1), for "open the file". */
export function rootOf(blocks: readonly DocBlock[]): string | null {
  const fm = blocks[0]
  return fm?.type === 'frontmatter' ? get(parseHeader(fm.props.yaml), 'root') : null
}

/** The anchor a `code:` href carries, or `null` for any other link (or a malformed one). */
export function codeAnchor(href: string): Anchor | null {
  if (!href.startsWith('code:')) return null
  const a = parseAnchor(href.slice('code:'.length))
  return typeof a === 'string' ? null : a
}

/** `Excerpt(path, start, text)` → `path#L<start>-L<start+lines-1>` (71 §2.3). */
export function excerptAnchor(p: ExcerptProps): Anchor & { kind: 'code'; lines: [number, number] } {
  const start = p.start ?? 1
  return { kind: 'code', path: p.path, lines: [start, start + excerptLines(p.text).length - 1] }
}

/** A diagram with addressable units (71 §2.4): the parsed root props, so the reader mounts the view itself. */
export type Diagram = { type: 'Graph'; props: GraphData } | { type: 'Sequence'; props: SequenceData }

/** What a `codeBlock` is to the reader (71 §2.2 block mapping). */
export type CodeKind =
  | { kind: 'excerpt'; props: ExcerptProps }
  | { kind: 'diagram'; diagram: Diagram } // `Graph` / `Sequence` root: full column width
  | { kind: 'surface'; code: string } // any other catalog root: prose width
  | { kind: 'invalid'; code: string; error: string }
  | { kind: 'mermaid'; code: string }
  | { kind: 'code'; lang: string; text: string }

export function classifyCode(block: Extract<DocBlock, { type: 'codeBlock' }>): CodeKind {
  const text = plainText(block.content)
  const lang = block.props.language
  if (lang === 'mermaid' || lang === 'mmd') return { kind: 'mermaid', code: text }
  if (lang !== 'json-render' && lang !== 'jsonrender') return { kind: 'code', lang, text }
  const parsed = parseSpec(text)
  if ('error' in parsed) return { kind: 'invalid', code: text, error: parsed.error }
  const root = parsed.spec.elements[parsed.spec.root]
  if (root?.type === 'Excerpt') return { kind: 'excerpt', props: root.props as ExcerptProps }
  if (root?.type === 'Graph') return { kind: 'diagram', diagram: { type: 'Graph', props: root.props as GraphData } }
  if (root?.type === 'Sequence') return { kind: 'diagram', diagram: { type: 'Sequence', props: root.props as SequenceData } }
  return { kind: 'surface', code: text }
}

export type BoardExcerpt = { blockId: string; props: ExcerptProps; anchor: Anchor & { kind: 'code'; lines: [number, number] } }

/** Every top-level `Excerpt` block in document order — the code windows the side pane can show. */
export function excerptsOf(blocks: DocBlock[]): BoardExcerpt[] {
  const out: BoardExcerpt[] = []
  for (const b of blocks) {
    if (b.type !== 'codeBlock') continue
    const c = classifyCode(b)
    if (c.kind === 'excerpt') out.push({ blockId: b.id, props: c.props, anchor: excerptAnchor(c.props) })
  }
  return out
}
