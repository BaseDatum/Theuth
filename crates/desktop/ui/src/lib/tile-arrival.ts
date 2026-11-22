// Where a `show_pty` job or a `show_note` document lands (211 §3, §5): a tile beside its chat when that chat is on
// screen in a tiling layout; anywhere else nothing opens and the transcript card is the way in. `openTile` does the
// placement, the selected-pane takeover and the dedupe (the same job or document again retargets its tile).

import { openJobLogTile, openJobTerminalTile, openNoteTile } from '@/components/tiles/open'
import { tileable } from '@/lib/tiles'

/** `PtyViewOpened`: job `job` beside `session`'s chat, labelled `title`: a PTY as its interactive terminal tile, a
 * background (`shell()`) job as its read-only live log tile. */
export function ptyViewOpened(e: { session: string; job: string; title: string; pty: boolean }, visible: boolean): void {
  if (!visible || !tileable(e.session)) return
  const ref = { session: e.session, job: e.job }
  if (e.pty) openJobTerminalTile(e.session, ref, e.title)
  else openJobLogTile(e.session, ref, e.title)
}

/** `NoteViewOpened`: document `doc`'s note tile beside `session`'s chat, scrolled to `block` when given. */
export function noteViewOpened(e: { session: string; doc: string; title: string; block?: string | null }, visible: boolean): void {
  if (!visible || !tileable(e.session)) return
  openNoteTile(e.session, { doc: e.doc, title: e.title, ...(e.block ? { block: e.block } : {}) })
}
