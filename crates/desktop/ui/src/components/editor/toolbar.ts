/** Pure pieces of the mobile toolbar (59 §10), kept React-free so vitest can run them in node. */

/** Hide the bar this long after the editor blurs, so a tap on one of its buttons does not hide it. */
export const GRACE_MS = 150

/** The toolbar's own focus state; `readonly` and an open sheet are decided by the caller. */
export type Visibility = 'hidden' | 'shown' | 'grace'

export function nextVisibility(v: Visibility, ev: 'focus' | 'blur' | 'elapsed'): Visibility {
  switch (ev) {
    case 'focus':
      return 'shown'
    case 'blur':
      return v === 'hidden' ? 'hidden' : 'grace'
    case 'elapsed':
      return v === 'grace' ? 'hidden' : v
  }
}

/**
 * `readonly` hides the bar entirely; an open sheet keeps it (the link sheet took the focus, the user is still
 * editing). The block-type picker is not a sheet: it lives inside the bar's focus guard and never blurs the editor.
 */
export const toolbarVisible = (v: Visibility, readonly: boolean, sheetOpen: boolean): boolean =>
  !readonly && (v !== 'hidden' || sheetOpen)

/** The "Aa" picker's chips in §10 order; `props` is what `updateBlock` needs beside `type`. Diagram = `codeBlock` + `language`. */
export const BLOCK_TYPES = [
  { key: 'paragraph', label: 'Text', type: 'paragraph' },
  { key: 'h1', label: 'Heading 1', type: 'heading', props: { level: 1 } },
  { key: 'h2', label: 'Heading 2', type: 'heading', props: { level: 2 } },
  { key: 'h3', label: 'Heading 3', type: 'heading', props: { level: 3 } },
  { key: 'bullet', label: 'Bullet list', type: 'bulletListItem' },
  { key: 'numbered', label: 'Numbered list', type: 'numberedListItem' },
  { key: 'check', label: 'Checklist', type: 'checkListItem' },
  { key: 'quote', label: 'Quote', type: 'quote' },
  { key: 'code', label: 'Code', type: 'codeBlock' },
  { key: 'diagram', label: 'Diagram', type: 'codeBlock', props: { language: 'mermaid' } },
] as const

export type BlockTypeKey = (typeof BLOCK_TYPES)[number]['key']

/**
 * Which chip the cursor's block is on: the chip whose `props` all match wins over the bare type (a mermaid code
 * block is `diagram`, any other `code`); `null` for blocks the picker does not offer (table, divider, frontmatter…).
 */
export function blockTypeKey(block: { type: string; props?: Record<string, unknown> }): BlockTypeKey | null {
  let bare: BlockTypeKey | null = null
  for (const t of BLOCK_TYPES) {
    if (t.type !== block.type) continue
    if (!('props' in t)) bare ??= t.key
    else if (Object.entries(t.props).every(([k, v]) => block.props?.[k] === v)) return t.key
  }
  return bare
}
