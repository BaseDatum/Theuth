/** 157 D4: what the notebook's right-hand column shows — its notes list, or one note's editor. */
export type PaneMode = { kind: 'list' } | { kind: 'note'; id: string }

/**
 * Pure: `?note=<id>` opens the pane on that note only when it is one of this notebook's notes (`members`, the item
 * ids) or the one just created here (`fresh`: its `notebook_item_add` may not have landed in the list yet). A foreign
 * or unknown id falls back to the list. `members === undefined` (items still loading) trusts the URL rather than
 * flashing the list first.
 */
export function paneMode(note: string | undefined, members: readonly string[] | undefined, fresh: string | null): PaneMode {
  if (!note) return { kind: 'list' }
  if (note === fresh || members === undefined || members.includes(note)) return { kind: 'note', id: note }
  return { kind: 'list' }
}
