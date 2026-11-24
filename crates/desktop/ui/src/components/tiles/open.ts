// Every tile kind's opener and props, apart from its renderer (`kinds/*`, loaded only by `registry.tsx`): triggers all
// over the chat import these without pulling a kind's body (a VNC client, xterm, the CodeMap) into their module graph.

import type { Item } from '@/components/chat/Attachments'
import { openPopout } from '@/lib/popout'
import { openTile, type Tile, tileable } from '@/lib/tiles'
import type { DiffSpec } from '@/rpc/bindings/method.artifact.input'
import { useSessions } from '@/store/sessions'

/** A tile kind's Open in new window when it has no window of its own (`TileFrame`'s `popOut`): the tile alone in
 * the generic tile window (`lib/popout.ts` kind `tile`), drawn by the same kind over the same props. */
export const popOutTile = (tile: Tile) => (): Promise<void> => openPopout({ kind: 'tile', tile: tile.kind, session: tile.session, props: tile.props })

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
/** `refresh`: only an open diagram tile takes the new markup (a redraw), staying behind its panel's active tab. */
export const openDiagramTile = (host: string, props: DiagramProps, refresh = false): string | null =>
  openTile({ kind: 'diagram', host, key: props.name, props, refresh })

/** An attachment's image or PDF (the desktop lightbox): the item, its chat, and the owning row's membership. */
export type MediaProps = { item: Item; session?: string; active: boolean }

/** One tile per removal target (two attachments of one path stay two), else per path. */
const mediaKey = (item: Item): string => (item.occurrence ? JSON.stringify(item.occurrence.target) : (item.path ?? item.name))

export const openMediaTile = (host: string, item: Item, session: string | undefined, active: boolean): string | null =>
  openTile({ kind: 'media', host, key: mediaKey(item), props: { item, session, active } satisfies MediaProps })

/** Items whose tile is open beside `host` take their current state (an occurrence removed since, the row folded); none
 * comes to the front of its panel. */
export function refreshMediaTiles(host: string, items: Item[], session: string | undefined, active: boolean): void {
  for (const item of items) openTile({ kind: 'media', host, key: mediaKey(item), props: { item, session, active } satisfies MediaProps, refresh: true })
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

/** A `job-terminal` tile: one job's PTY (`JobCard`'s Open terminal, a `show_pty` call) and its label. */
export type JobTerminalProps = JobRef & { label: string }
/** `job`'s terminal beside the chat in leaf `host`, labelled `label` (its name, command or `show_pty` title); each
 * job its own tile, so the same job again retargets the open one. */
export const openJobTerminalTile = (host: string, job: JobRef, label: string): string | null =>
  openTile({
    kind: 'job-terminal',
    host,
    key: `${job.session}/${job.job}`,
    props: { session: job.session, job: job.job, label } satisfies JobTerminalProps,
  })

/** A `job-log` tile (211 §5): a background (`shell()`) job's live output, read-only, and its label. */
export type JobLogProps = JobRef & { label: string }
/** `job`'s live log beside the chat in leaf `host`; each job its own tile. */
export const openJobLogTile = (host: string, job: JobRef, label: string): string | null =>
  openTile({ kind: 'job-log', host, key: `${job.session}/${job.job}`, props: { session: job.session, job: job.job, label } satisfies JobLogProps })

/** A `note` tile (211 §3): document `doc` (a note, board, knowledge page or meeting note) live in its editor; `block`
 * is scrolled into view, `title` (the `show_note` title) names the tile where its body is not drawn. */
export type NoteProps = { doc: string; block?: string; title?: string }
/** Document `doc` beside the chat in leaf `host`; one tile per document, so the same one again retargets it. */
export const openNoteTile = (host: string, note: NoteProps): string | null => openTile({ kind: 'note', host, key: note.doc, props: note })

/** A `file` tile (212 §4): `show_file`'s file `path` (absolute) of `session`, fetched by the tile with
 * `file_view_read`; `line..end_line` scrolled to and marked; `id` is the tool's descriptor id. */
export type FileProps = { session: string; path: string; id: string; title: string; lang?: string; line?: number; end_line?: number }
/** The file beside the chat in leaf `host`; one tile per `id` (session + path), so the same file again retargets it. */
export const openFileTile = (host: string, file: FileProps): string | null => openTile({ kind: 'file', host, key: file.id, props: file })

/** A `diff` tile (212 §4): `show_diff`'s `spec` of `session`, fetched by the tile with `diff_view_read`. */
export type DiffProps = { session: string; id: string; title: string; spec: DiffSpec }
/** The diff beside the chat in leaf `host`; one tile per `id` (session + spec). */
export const openDiffTile = (host: string, diff: DiffProps): string | null => openTile({ kind: 'diff', host, key: diff.id, props: diff })

/** A sandbox VM's live desktop; one tile per VM name. */
export type VmProps = { name: string }
export const openVmTile = (host: string, name: string): string | null => openTile({ kind: 'vm', host, key: name, props: { name } satisfies VmProps })
