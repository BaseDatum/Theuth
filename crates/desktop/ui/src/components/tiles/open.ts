// Every tile kind's opener and props, apart from its renderer (`kinds/*`, loaded only by `registry.tsx`): triggers all
// over the chat import these without pulling a kind's body (a VNC client, xterm, the CodeMap) into their module graph.

import type { Item } from '@/components/chat/Attachments'
import { openTile, openTileOf, tileable } from '@/lib/tiles'
import type { JobView } from '@/rpc/bindings/method.agent.output'
import { useSessions } from '@/store/sessions'

/** The artifact (or app draft) the session's one artifact tile shows. */
export type ArtifactProps = { callId: string }

/** Open the session's artifact tile beside `host`, or retarget the open one, on `callId`. */
export const openArtifactTile = (host: string, callId: string): string | null =>
  openTile({ kind: 'artifact', host, props: { callId } satisfies ArtifactProps })

/** Show `callId`: a tile where `session`'s chat is in a pane of a tiling layout, else the artifact sheet. */
export function showArtifact(session: string, callId: string): void {
  if (tileable(session)) openArtifactTile(session, callId)
  else useSessions.getState().setArtifactPanel(session, callId)
}

/** The session's live headless browser; one per chat. */
export type BrowserProps = Record<string, never>
export const openBrowserTile = (host: string): string | null => openTile({ kind: 'browser', host, props: {} satisfies BrowserProps })

/** The chat's CodeMap (73 §6.7), one per chat; `root` is the code root it shows. */
export type CodemapProps = { root: string }
export const openCodemapTile = (host: string, root: string): string | null =>
  openTile({ kind: 'codemap', host, props: { root } satisfies CodemapProps })

/** A diagram's fullscreen: the svg markup (mermaid's, or the mounted React svg), the export stem (a content hash, so
 * also the tile key) and the card's label. */
export type DiagramProps = { svg: string; name: string; label: string }
export const openDiagramTile = (host: string, props: DiagramProps): string | null => openTile({ kind: 'diagram', host, key: props.name, props })

/** An attachment's image or PDF (the desktop lightbox): the item, its chat, and the owning row's membership. */
export type MediaProps = { item: Item; session?: string; active: boolean }

/** One tile per removal target (two attachments of one path stay two), else per path. */
const mediaKey = (item: Item): string => (item.occurrence ? JSON.stringify(item.occurrence.target) : (item.path ?? item.name))

export const openMediaTile = (host: string, item: Item, session: string | undefined, active: boolean): string | null =>
  openTile({ kind: 'media', host, key: mediaKey(item), props: { item, session, active } satisfies MediaProps })

/** Items whose tile is open beside `host` take their current state (an occurrence removed since, the row folded). */
export function refreshMediaTiles(host: string, items: Item[], session: string | undefined, active: boolean): void {
  for (const item of items) if (openTileOf('media', host, mediaKey(item))) openMediaTile(host, item, session, active)
}

/** The Context panel beside the chat (or draft) in leaf `host`; one per chat. */
export const openMemoryTile = (host: string): string | null => openTile({ kind: 'memory', host, props: {} })

/** The system prompt beside the chat in leaf `host`; one per chat. */
export const openPromptTile = (host: string): string | null => openTile({ kind: 'prompt', host, props: {} })

/** The background jobs beside the chat in leaf `host`; one per chat. */
export const openJobsTile = (host: string): string | null => openTile({ kind: 'jobs', host, props: {} })

export type SkillProps = { name: string }
/** Skill `name` beside the chat in leaf `host`; one tile per skill. */
export const openSkillTile = (host: string, name: string): string | null => openTile({ kind: 'skill', host, key: name, props: { name } satisfies SkillProps })

export type JobRef = { session: string; job: string }

/** The `terminals` tile: the picked terminal, until one is picked the session's first. */
export type TerminalsProps = { job?: JobRef }
/** The terminals beside the chat in leaf `host`; one per chat. */
export const openTerminalsTile = (host: string): string | null => openTile({ kind: 'terminals', host, props: {} satisfies TerminalsProps })

/** A `job-terminal` tile: one job's PTY (`JobCard`'s Open terminal) and its status label. */
export type JobTerminalProps = JobRef & { label: string }
/** `job`'s terminal beside the chat in leaf `host`; each job its own tile. */
export const openJobTerminalTile = (host: string, job: JobView): string | null =>
  openTile({
    kind: 'job-terminal',
    host,
    key: `${job.session}/${job.id}`,
    props: { session: job.session, job: job.id, label: job.name ?? job.command } satisfies JobTerminalProps,
  })

/** A sandbox VM's live desktop; one tile per VM name. */
export type VmProps = { name: string }
export const openVmTile = (host: string, name: string): string | null => openTile({ kind: 'vm', host, key: name, props: { name } satisfies VmProps })
