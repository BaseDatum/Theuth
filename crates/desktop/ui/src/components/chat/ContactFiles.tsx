// 199 CONTACT-FILES: file cards (D12) and the human answer's attach control (D15). A card never loads the bytes: its
// thumbnail is `contact_file_thumbnail(holder, id)` (the holder's bounded decoder), its actions name paths.
import { open as pickFiles, save } from '@tauri-apps/plugin-dialog'
import { openPath, revealItemInDir } from '@tauri-apps/plugin-opener'
import { useQuery } from '@tanstack/react-query'
import { File, FileArchive, FileText, FileX, Image as ImageIcon, Paperclip, X } from 'lucide-react'
import type { ReactNode } from 'react'
import { toast } from 'sonner'
import { create } from 'zustand'
import { errorText, useAction } from '@/components/settings/shared'
import { useMesh } from '@/components/tasks/queries'
import { Button } from '@/components/ui/button'
import { fmtBytes, isLocal } from '@/lib/files'
import type { FileNote } from '@/lib/injected'
import { engineLabel } from '@/lib/owner'
import { container } from '@/lib/platform'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { useEngines } from '@/link/engines'
import type { Api } from '@/rpc/bindings/client'
import type { ContactFile, PeerReceipt } from '@/rpc/bindings/method.agent.output'
import { useQuerySupport } from '@/rpc/capabilities'
import { fileKey } from '@/store/reduce'
import { type Entry, useSessions } from '@/store/sessions'
import { Clock } from './MessagePeer'

const TOUCH = 'pointer-coarse:min-h-11'

/** One file as its card shows it, in either direction. `state` is `InState` or `OutState` by `dir`; `peer` says whom
 * it is from or to; `path` is the holder's (in: saved, out: the original). */
export type FileView = {
  holder: string
  id: string
  dir: 'in' | 'out'
  state: ContactFile['state']
  name: string
  type: string
  size?: number
  done: number
  peer: string
  path?: string
  reason?: string
  machine?: string
}

const TERMINAL: Record<string, true> = { received: true, failed: true, sent: true }
const basename = (path: string) => path.split(/[\\/]/).pop() || path

/** A holder record as its card; `path` is the original an outgoing record does not carry. */
export function recordView(f: ContactFile, path?: string): FileView {
  const at = f.path ?? path
  return {
    holder: f.holder,
    id: f.id,
    dir: f.direction,
    state: f.state,
    name: f.name,
    type: f.media_type,
    ...(f.size == null ? {} : { size: f.size }),
    done: f.done,
    peer: `${f.direction === 'in' ? 'from' : 'to'} ${f.contact}`,
    ...(at ? { path: at } : {}),
    ...(f.error ? { reason: f.error } : {}),
    ...(f.machine ? { machine: f.machine } : {}),
  }
}

/**
 * An incoming file's card: the element's note, overlaid by what is known since — the file's notice row (terminal),
 * then its holder record (`ContactFileChanged` live, else `contact_file` on load). A record still moving never hides a
 * notice that already ended it.
 */
export function fileView(note: FileNote, record?: ContactFile, notice?: FileNote): FileView {
  const end = notice ?? note
  const size = end.bytes ?? note.bytes
  const base: FileView = {
    holder: note.holder,
    id: note.id,
    dir: 'in',
    state: end.state,
    name: note.name,
    type: note.type,
    ...(size === undefined ? {} : { size }),
    done: 0,
    peer: `from ${note.from.replace(/^(peer|contact):/, '')}`,
    ...(end.saved ? { path: end.saved } : {}),
    ...(end.reason ? { reason: end.reason } : {}),
    ...(end.machine ? { machine: end.machine } : {}),
  }
  if (!record || record.direction !== 'in' || (notice && !TERMINAL[record.state])) return base
  return { ...base, ...recordView(record) }
}

/**
 * The files a send offered, from the receipts' records (`peer_message` → `PeerRecord.files`, in message order) with
 * each `ContactFileChanged` over them; an outgoing record of one of `exchanges` the records do not list yet comes last.
 */
export function sentFiles(exchanges: readonly string[], records: readonly (readonly ContactFile[] | undefined)[], live: ReadonlyMap<string, ContactFile> | undefined): ContactFile[] {
  const out = new Map<string, ContactFile>()
  for (const f of records.flat()) if (f && f.direction === 'out') out.set(fileKey(f.holder, f.id), f)
  for (const [k, f] of live ?? []) if (f.direction === 'out' && (out.has(k) || exchanges.includes(f.exchange))) out.set(k, f)
  return [...out.values()]
}

/** `38% · 456 MB of 1.2 GB`; `null` without a size. */
const progress = (v: FileView) => (v.size ? { pct: Math.min(100, Math.round((v.done / v.size) * 100)), text: `${fmtBytes(v.done)} of ${fmtBytes(v.size)}` } : null)

/** The state line's words and tone. */
function stateLine(v: FileView, elsewhere?: string): { text: string; tone: 'ok' | 'busy' | 'bad' } {
  const p = progress(v)
  switch (v.state) {
    case 'received':
      return { text: elsewhere ? `Received · stored on ${elsewhere}` : 'Received', tone: 'ok' }
    case 'sent':
      return { text: `Sent · ${v.peer.replace(/^to /, '')}'s machine received it`, tone: 'ok' }
    case 'failed':
      return { text: 'Failed', tone: 'bad' }
    case 'receiving':
      return { text: p && v.done ? `Receiving · ${p.pct}%` : 'Receiving', tone: 'busy' }
    case 'sending':
      return { text: p && v.done ? `Sending · ${p.pct}% · ${p.text}` : 'Sending', tone: 'busy' }
    case 'publishing':
      return { text: 'Saving', tone: 'busy' }
    case 'saved':
      return { text: 'Saved', tone: 'busy' }
    case 'queued':
      return { text: 'Queued', tone: 'busy' }
  }
}

const TONE = { ok: 'text-success', busy: 'text-info', bad: 'text-danger' } as const

function TypeIcon({ type, failed }: { type: string; failed: boolean }) {
  const Icon = failed ? FileX : type.startsWith('image/') ? ImageIcon : /zip|tar|compress|archive|7z|rar/.test(type) ? FileArchive : type.startsWith('text/') || type === 'application/pdf' ? FileText : File
  return <Icon className={cn('size-8 shrink-0', failed ? 'text-danger' : 'text-fg-secondary')} aria-hidden="true" />
}

/** D12: a received image's thumbnail from its holder; the type icon while it loads, when it fails, or for any other file. */
function Thumb({ v }: { v: FileView }) {
  const can = useQuerySupport(['contact_file_thumbnail'])
  const api = useApi()
  const image = v.type.startsWith('image/') && v.state === 'received' && !!v.holder
  const thumb = useQuery({
    queryKey: queryKeys.contactFileThumbnail(v.holder, v.id),
    queryFn: () => api.contact_file_thumbnail(v.holder, v.id),
    enabled: can && image,
    staleTime: Infinity,
    retry: false,
  })
  return thumb.data && image ? (
    <img src={`data:${thumb.data.mime};base64,${thumb.data.data}`} alt={`Thumbnail of ${v.name}`} className="w-33 rounded-lg bg-layer-2" />
  ) : (
    <TypeIcon type={v.type} failed={v.state === 'failed'} />
  )
}

/** P2 "Fetch here": the local copy's path by `holder/id`, kept beside the transcript so a re-mounted card keeps it. */
export const useFetched = create<Record<string, string>>()(() => ({}))

const fail = (e: unknown) => toast.error(errorText(e))

/**
 * Per machine (D12, D15): on the holder from this desktop's own engine, Open, Reveal, Save as… (`contact_file_export`)
 * and Copy path; from another of our machines "stored on <machine>" and Fetch here (`contact_file_fetch`), then the
 * local actions on the copy; a phone, an iPad or a linked engine gets the path only. Outgoing: the original's actions.
 */
function Actions({ v, session, here, local }: { v: FileView; session?: string; here: boolean; local: boolean }) {
  const api = useApi()
  const key = fileKey(v.holder, v.id)
  const copy = useFetched((s) => s[key])
  const fetch = useAction(() => api.contact_file_fetch(session as string, v.holder, v.id), { onSuccess: (path) => useFetched.setState({ [key]: path }) })
  if (v.dir === 'in' && v.state !== 'received') return null
  const at = here ? v.path : copy
  const shown = at ?? v.path
  const reveal = container() === 'macos' ? 'Reveal in Finder' : 'Show in file manager'
  const buttons: ReactNode[] = []
  const add = (label: string, run: () => void, busy?: boolean) =>
    buttons.push(
      <Button key={label} type="button" variant="outline" size="xs" className={TOUCH} disabled={busy} aria-busy={busy || undefined} onClick={run}>
        {label}
      </Button>,
    )
  if (v.dir === 'in' && !here && local && session && !copy) add('Fetch here', () => fetch.mutate(undefined), fetch.isPending)
  if (local && at) {
    add('Open', () => void openPath(at).catch(fail))
    add(reveal, () => void revealItemInDir(at).catch(fail))
  }
  if (v.dir === 'in' && here && local)
    add('Save as…', () =>
      void save({ defaultPath: v.name })
        .then((dest) => (dest ? api.contact_file_export(v.holder, v.id, dest).then((p) => toast(`Saved ${p}`)) : undefined))
        .catch(fail),
    )
  if (shown) add('Copy path', () => void navigator.clipboard.writeText(shown).then(() => toast.message('Copied'), fail))
  return buttons.length ? <div className="col-start-2 mt-1 flex flex-wrap gap-1.5">{buttons}</div> : null
}

/** One file card: thumbnail or type icon, name, size · type · whom, the state (with progress while bytes move), the
 * path, the reason, then the actions this machine has. `ended`: an ended share's notice (D17), never given to the agent. */
export function FileCard({ v, session, ended }: { v: FileView; session?: string; ended?: boolean }) {
  const self = useMesh().data
  const local = isLocal(useEngines((s) => s.active))
  const here = !!v.holder && v.holder === self?.self_id
  const elsewhere = v.dir === 'in' && v.holder && !here ? v.machine || engineLabel(v.holder, self?.peers) : undefined
  const line = stateLine(v, elsewhere)
  const p = v.state === 'receiving' || v.state === 'sending' ? progress(v) : null
  const failed = v.state === 'failed'
  return (
    <div
      className={cn(
        'grid grid-cols-[auto_minmax(0,1fr)] items-start gap-x-3 gap-y-1 rounded-lg border p-2.5',
        failed ? 'border-danger-border bg-danger-bg/50' : 'border-border-subtle bg-layer-2/45',
      )}
    >
      <Thumb v={v} />
      <div className="flex min-w-0 flex-col gap-0.5">
        <span className="text-[13px] font-medium wrap-anywhere">{v.name}</span>
        <span className="flex flex-wrap gap-1.5 text-[11px] text-fg-secondary">
          {[v.size === undefined ? undefined : fmtBytes(v.size), v.type, v.peer].filter(Boolean).join(' · ')}
        </span>
        <span className={cn('inline-flex flex-wrap items-center gap-1.5 text-[11px] font-medium', TONE[line.tone])}>
          <span className="size-1.5 rounded-full bg-current" aria-hidden="true" />
          {line.text}
          {ended ? <span className="font-normal text-fg-secondary">· not given to the agent</span> : null}
        </span>
        {p ? (
          <div className="mt-0.5 h-1.5 overflow-hidden rounded-full bg-layer-3" role="progressbar" aria-label={`${v.name} ${v.dir === 'in' ? 'received' : 'sent'}`} aria-valuemin={0} aria-valuemax={100} aria-valuenow={p.pct}>
            <span className="block h-full rounded-full bg-info" style={{ width: `${p.pct}%` }} />
          </div>
        ) : null}
        {v.path ? <span className="font-mono text-[10.5px] text-fg-tertiary wrap-anywhere">{v.path}</span> : null}
        {v.reason ? <span className="text-[11.5px] text-danger wrap-anywhere">{v.reason}</span> : null}
      </div>
      <Actions v={v} session={session} here={here} local={local} />
    </div>
  )
}

type FileEntry = Extract<Entry, { kind: 'file' }>

/** The newest notice row for `note`'s file in `entries`; stable while the transcript streams (the entry object). */
const noticeFor = (entries: readonly Entry[] | undefined, note: FileNote): FileEntry | undefined =>
  entries?.findLast((x): x is FileEntry => x.kind === 'file' && x.file.holder === note.holder && x.file.id === note.id)

/** An incoming file from its note, with its live record, notice row and (on load, while neither is known) `contact_file`. */
export function IncomingFile({ note, session, ended }: { note: FileNote; session: string; ended?: boolean }) {
  const can = useQuerySupport(['contact_file'])
  const api = useApi()
  const live = useSessions((s) => s.sessions[session]?.files.get(fileKey(note.holder, note.id)))
  const notice = useSessions((s) => noticeFor(s.sessions[session]?.entries, note))
  const fetched = useQuery({
    queryKey: queryKeys.contactFile(note.holder, note.id),
    queryFn: () => api.contact_file(note.holder, note.id),
    enabled: can && !live && !notice && note.state === 'receiving',
    staleTime: Infinity,
    retry: false,
  })
  return <FileCard v={fileView(note, live ?? fetched.data, notice?.file ?? (note.state === 'receiving' ? undefined : note))} session={session} ended={ended} />
}

/** A message card's files (its placeholders), outside the clamped body. */
export function PeerFiles({ files, session }: { files: readonly FileNote[]; session: string }) {
  return (
    <div className="flex flex-col gap-2.5 px-4 pt-3 pb-1">
      {files.map((f) => (
        <IncomingFile key={fileKey(f.holder, f.id)} note={f} session={session} />
      ))}
    </div>
  )
}

/** A notice row from theuth (or an ended share's `errors` row): a file card under a `theuth` line, never a `you` bubble. */
export function FileNoticeCard({ note, session, at, ended }: { note: FileNote; session: string; at?: number; ended?: boolean }) {
  return (
    <div className="my-2 flex flex-col gap-1.5" role="note">
      <div className="flex items-center gap-2 text-[11px] text-fg-tertiary">
        File notice from theuth
        <Clock at={at} />
      </div>
      <IncomingFile note={note} session={session} ended={ended} />
    </div>
  )
}

/** SendCard's files: the offered records, or the call's own paths (queued) before any record is known. */
export function SentFiles({ files, paths, session }: { files: readonly ContactFile[]; paths: readonly string[]; session: string }) {
  const views = files.length
    ? files.map((f) => recordView(f, paths.find((p) => basename(p) === f.name)))
    : paths.map((p): FileView => ({ holder: '', id: p, dir: 'out', state: 'queued', name: basename(p), type: '', done: 0, peer: '', path: p }))
  return (
    <div className="mt-2.5 flex flex-col gap-2.5">
      {views.map((v) => (
        <FileCard key={fileKey(v.holder, v.id)} v={v} session={session} />
      ))}
    </div>
  )
}

/** D15: `answer_peer_request` with the picked files. The `files` parameter is E1's (engine P1) and not in the generated
 * bindings yet, so it rides as the 4th positional param, sent only when there are files. */
export function answerPeer(api: Api, session: string, replyTo: string, text: string, files: readonly string[]): Promise<PeerReceipt> {
  if (!files.length) return api.answer_peer_request(session, replyTo, text)
  return (api.answer_peer_request as unknown as (...args: unknown[]) => Promise<PeerReceipt>)(session, replyTo, text, files)
}

/**
 * D15: file chips for a human answer, only on the chat's machine — the desktop shell on the engine that owns the chat,
 * where the gate can open the picked paths. Elsewhere (another machine owns it, or a phone / iPad / linked engine
 * controls it) the control says "attach on <machine>".
 */
export function AttachFiles({ session, files, onChange, disabled }: { session: string; files: readonly string[]; onChange: (files: string[]) => void; disabled?: boolean }) {
  const can = useQuerySupport(['list_sessions'])
  const api = useApi()
  const mesh = useMesh().data
  const local = isLocal(useEngines((s) => s.active))
  const { data: rows } = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions(), enabled: can })
  const row = rows?.find((r) => r.id === session)
  const owner = row && !row.here ? engineLabel(row.owner, mesh?.peers) : undefined
  const attach = local && !owner
  return (
    <div className="flex flex-wrap items-center gap-1.5">
      {files.map((p) => (
        <span key={p} title={p} className="inline-flex max-w-full items-center gap-1 rounded-full border border-border-subtle bg-layer-2 py-0.5 pr-1 pl-2 text-[11.5px]">
          <Paperclip className="size-3 shrink-0" aria-hidden="true" />
          <span className="truncate">{basename(p)}</span>
          <button type="button" className="rounded-full p-0.5 hover:bg-layer-3" aria-label={`Remove ${basename(p)}`} disabled={disabled} onClick={() => onChange(files.filter((f) => f !== p))}>
            <X className="size-3" aria-hidden="true" />
          </button>
        </span>
      ))}
      {attach ? (
        <Button
          type="button"
          variant="ghost"
          size="xs"
          className={TOUCH}
          disabled={disabled}
          onClick={() =>
            void pickFiles({ multiple: true }).then((picked) => {
              if (picked?.length) onChange([...new Set([...files, ...picked])])
            }, fail)
          }
        >
          <Paperclip aria-hidden="true" /> Attach files
        </Button>
      ) : (
        <span className="text-xs text-fg-tertiary">attach on {owner ?? mesh?.self_name ?? "the chat's machine"}</span>
      )}
    </div>
  )
}
