// What each tray item (a dash beside the grabber, a tile of the opened tray) becomes when dragged into a pane
// (`usePayloadDrag`): the tile its click opens beside the chat, under the same id, so a drop of an open one moves it.

import type { ArtifactProps, BrowserProps, CodemapProps, JobRef, JobTerminalProps, VmProps, WhiteboardProps } from '@/components/tiles/open'
import type { DragPayload } from '@/lib/panes'
import { useWebViews, webLeaf } from '@/lib/web-views'
import type { BrowserTarget } from './BrowserTargets'

export type TrayDrag = { payload: DragPayload; title: string; prepare?: () => void }

const tile = (session: string, tileKind: string, props: unknown, title: string, key?: string): TrayDrag => ({
  payload: { kind: 'tile', tileKind, props, title, session, ...(key ? { key } : {}) },
  title,
})

/** `showArtifact`'s tile. */
export const artifactDrag = (session: string, callId: string, title: string): TrayDrag => tile(session, 'artifact', { callId } satisfies ArtifactProps, title)

/** `openWhiteboardTile`'s tile. */
export const whiteboardDrag = (session: string, w: { doc: string; title: string }): TrayDrag =>
  tile(session, 'whiteboard', { doc: w.doc, title: w.title } satisfies WhiteboardProps, w.title || 'Whiteboard', w.doc)

/** `openVmTile`'s tile. */
export const vmDrag = (session: string, name: string): TrayDrag => tile(session, 'vm', { name } satisfies VmProps, `Desktop of ${name}`, name)

/** `openJobsTile`'s tile. */
export const jobsDrag = (session: string): TrayDrag => tile(session, 'jobs', {}, 'Jobs')

/** `openTerminalsTile`'s tile. */
export const terminalsDrag = (session: string): TrayDrag => tile(session, 'terminals', {}, 'Terminals')

/** `openJobTerminalTile`'s tile: one job's terminal, owned by the chat `owner` whose tray shows it. */
export const terminalDrag = (owner: string, job: JobRef, label: string): TrayDrag =>
  tile(owner, 'job-terminal', { session: job.session, job: job.job, label } satisfies JobTerminalProps, label, `${job.session}/${job.job}`)

/** `openCodemapTile`'s tile. */
export const codemapDrag = (session: string, root: string): TrayDrag => tile(session, 'codemap', { root } satisfies CodemapProps, 'Code map')

/** The agent's browser tile, or a `show_web` page's frame: recorded in the web views on drop, as `openWebView` does. */
export function browserDrag(session: string, t: BrowserTarget): TrayDrag {
  if (t.kind === 'agent') return tile(session, 'browser', {} satisfies BrowserProps, 'Browser')
  return {
    payload: { kind: 'chat', session: webLeaf(t.id) },
    title: t.title || t.url,
    prepare: () => {
      const store = useWebViews.getState()
      if (!store.views[t.id]) store.open({ id: t.id, session, url: t.url, title: t.title })
    },
  }
}
