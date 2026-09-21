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

/** `readonly` hides the bar entirely; an open sheet keeps it (the sheet took the focus, the user is still editing). */
export const toolbarVisible = (v: Visibility, readonly: boolean, sheetOpen: boolean): boolean =>
  !readonly && (v !== 'hidden' || sheetOpen)

/** The "Aa" sheet's rows in §10 order; `props` is what `updateBlock` needs beside `type`. */
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
] as const

export type BlockTypeKey = (typeof BLOCK_TYPES)[number]['key']

/** Which sheet row the cursor's block is on; `null` for blocks the sheet does not offer (table, divider, frontmatter…). */
export function blockTypeKey(block: { type: string; props?: Record<string, unknown> }): BlockTypeKey | null {
  for (const t of BLOCK_TYPES) {
    if (t.type !== block.type) continue
    if (!('props' in t) || t.props.level === block.props?.level) return t.key
  }
  return null
}
