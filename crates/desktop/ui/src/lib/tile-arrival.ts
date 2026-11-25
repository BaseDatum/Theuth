// Where a `show_pty` job, a `show_note` document (211 §3, §5) or a `show_file` file / `show_diff` diff (212 §4) lands:
// a tile beside its chat when that chat is on screen in a tiling layout; anywhere else nothing opens and the transcript
// card is the way in. `openTile` does the placement, the selected-pane takeover and the dedupe (the same job, document,
// file or diff again retargets its tile).

import { openDiffTile, openFileTile, openJobLogTile, openJobTerminalTile, openNoteTile, openWhiteboardTile } from '@/components/tiles/open'
import type { WhiteboardViewOpened } from '@/components/whiteboard/wire'
import { tileable } from '@/lib/tiles'
import type { DiffSpec } from '@/rpc/bindings/method.artifact.input'

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

/**
 * `WhiteboardViewOpened` (218 §8.8): whiteboard `doc`'s tile beside `session`'s chat when that chat is on screen in a
 * tiling layout; anywhere else (a phone, a hidden chat, no pane) the whiteboard's own page, bound to `session`.
 * `go` pushes an app path (the router lives in `App.tsx`).
 */
export function whiteboardViewOpened(e: WhiteboardViewOpened, visible: boolean, go: (href: string) => void): void {
  if (visible && tileable(e.session) && openWhiteboardTile(e.session, { doc: e.doc, title: e.title }) !== null) return
  go(`/whiteboards/${encodeURIComponent(e.doc)}?session=${encodeURIComponent(e.session)}`)
}

/** `FileViewOpened` (212 §4): the file's tile beside `session`'s chat, keyed by the descriptor id, scrolled to `line`
 * and marking `line..end_line` when given (a null bound is left out). */
export function fileViewOpened(
  e: { session: string; id: string; path: string; title: string; lang: string; line?: number | null; end_line?: number | null },
  visible: boolean,
): void {
  if (!visible || !tileable(e.session)) return
  openFileTile(e.session, {
    session: e.session,
    path: e.path,
    id: e.id,
    title: e.title,
    lang: e.lang,
    ...(e.line != null ? { line: e.line } : {}),
    ...(e.end_line != null ? { end_line: e.end_line } : {}),
  })
}

/** `DiffViewOpened` (212 §4): the diff's tile beside `session`'s chat, keyed by the descriptor id; it fetches `spec`. */
export function diffViewOpened(e: { session: string; id: string; title: string; spec: DiffSpec }, visible: boolean): void {
  if (!visible || !tileable(e.session)) return
  openDiffTile(e.session, { session: e.session, id: e.id, title: e.title, spec: e.spec })
}
