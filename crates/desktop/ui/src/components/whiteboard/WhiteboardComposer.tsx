// 218 composer: the session view's own `Composer` (dictation, voice notes, attachments, model and effort, skills,
// send/stop, keys, paste), docked bottom right on the whiteboard at iPhone width. Its toolbar leads with the tools
// button (the ink panel with Hand, and the cluster's actions); just above the box sit the session picker and the
// agent's one-line status, then the tray — what the send carries besides the words (one thumbnail per image, each
// removable; marks off screen as an opt-in). The send itself is the whiteboard's (`send/send.ts`) through `deliver`.

import { useQuery } from '@tanstack/react-query'
import { Check, ExternalLink, Link2, Plus, Unlink, X } from 'lucide-react'
import { type ReactNode, type Ref, useEffect, useState } from 'react'
import { Composer } from '@/components/chat/Composer'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { useApi } from '@/lib/rpc'
import { useDark } from '@/lib/theme'
import { useQuerySupport } from '@/rpc/capabilities'
import { useSessions } from '@/store/sessions'
import type { PlannedImage, SendPlan } from './canvas/api'
import type { Board } from './canvas/model'
import { type ClusterProps, ToolsMenu, useCloseOnCanvas } from './Cluster'
import { imageKey } from './send/send'
import { inkCss } from './ink/colour'
import type { WhiteboardSession } from './wire'

/** A session's name for the picker: its title, cut to fit. */
export function shortName(title: string | null | undefined, max = 22): string {
  const t = (title ?? '').trim() || 'Untitled session'
  return t.length > max ? `${t.slice(0, max - 1)}…` : t
}

export type SessionPick = {
  session: string | null
  title: string | null
  bound: WhiteboardSession[]
  busy: boolean
  onPick(session: string): void
  onNew(): void
  onDetach(session: string): void
  /** Open the session in chat (leaves the whiteboard). */
  onOpen?(session: string): void
}

export function useSessionTitle(session: string | null, bound: WhiteboardSession[]): string | null {
  const live = useSessions((s) => (session ? s.sessions[session]?.title : undefined))
  return session ? live || bound.find((b) => b.session === session)?.title || null : null
}

/** The session picker: New session, this whiteboard's sessions, recent others, detach. */
export function SessionChip({ p }: { p: SessionPick }) {
  const dark = useDark()
  const [open, setOpen] = useState(false)
  useCloseOnCanvas(open, () => setOpen(false))
  const api = useApi()
  const can = useQuerySupport(['list_sessions'])
  const recent = useQuery({ enabled: can && open, queryKey: ['whiteboard', 'recent-sessions'], queryFn: () => api.list_sessions(), staleTime: 15_000 })
  const boundIds = new Set(p.bound.map((b) => b.session))
  const others = (recent.data ?? [])
    .filter((s) => !s.archived && !s.parent && !boundIds.has(s.id) && s.kind !== 'observed')
    .sort((a, b) => b.updated_at.localeCompare(a.updated_at))
    .slice(0, 8)
  const row = (key: string, label: ReactNode, on: boolean, onClick: () => void, icon?: ReactNode) => (
    <li key={key}>
      <button type="button" onClick={() => (onClick(), setOpen(false))} className="wb-press flex min-h-11 w-full items-center gap-2 rounded-xl px-3 text-left text-sm">
        <span className="grid size-4 place-items-center">{on ? <Check className="size-4" /> : icon}</span>
        <span className="min-w-0 flex-1 truncate">{label}</span>
      </button>
    </li>
  )
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          aria-label={p.session ? `Session: ${p.title ?? 'untitled'}. Change session` : 'New session. Choose a session'}
          className="wb-glass flex h-8 min-w-0 max-w-[14rem] shrink items-center gap-1.5 rounded-full px-2.5 text-[12px] font-semibold"
        >
          <span className="wb-dot" data-busy={p.busy} style={p.session ? { background: inkCss('green', dark) } : undefined} aria-hidden />
          <span className="min-w-0 truncate">{p.session ? shortName(p.title, 28) : 'New session'}</span>
          {p.session && <Link2 className="size-3.5 shrink-0 opacity-60" aria-hidden />}
        </button>
      </PopoverTrigger>
      <PopoverContent side="top" align="start" collisionPadding={12} className="wb-glass w-72 rounded-2xl border-0 p-1.5">
        <ul className="flex flex-col" aria-label="Sessions">
          {row('new', 'New session', p.session === null, p.onNew, <Plus className="size-4" />)}
          {p.bound.length > 0 && <li className="wb-muted px-3 pt-1 pb-0.5 text-[11px] font-semibold uppercase tracking-wide">On this whiteboard</li>}
          {p.bound.map((b) => row(b.session, shortName(b.title, 34), b.session === p.session, () => p.onPick(b.session)))}
          {others.length > 0 && <li className="wb-muted px-3 pt-2 pb-0.5 text-[11px] font-semibold uppercase tracking-wide">Other sessions</li>}
          {others.map((s) => row(s.id, shortName(s.title || s.name, 34), false, () => p.onPick(s.id)))}
          {p.session && (
            <>
              <li className="my-1 h-px bg-current opacity-10" aria-hidden />
              {p.onOpen && row('open', 'Open in chat', false, () => p.onOpen?.(p.session as string), <ExternalLink className="size-4" />)}
              {row('detach', 'Detach this session', false, () => p.onDetach(p.session as string), <Unlink className="size-4" />)}
            </>
          )}
        </ul>
      </PopoverContent>
    </Popover>
  )
}

function Thumb({ image, board, compose, onRemove }: { image: PlannedImage; board: Board; compose(i: PlannedImage): Promise<Blob>; onRemove(): void }) {
  const [url, setUrl] = useState<string | null>(null)
  useEffect(() => {
    let live = true
    let made: string | null = null
    compose(image).then(
      (b) => {
        if (!live) return
        made = URL.createObjectURL(b)
        setUrl(made)
      },
      () => {},
    )
    return () => {
      live = false
      if (made) URL.revokeObjectURL(made)
    }
  }, [image, compose])
  const name = image.card ? board.cards[image.card]?.title || 'Card' : 'Canvas'
  const label = `${image.zoomOf ? 'Close-up of ' : ''}${name}, ${image.marks.length} ${image.marks.length === 1 ? 'mark' : 'marks'}`
  return (
    <li className="relative h-[60px] w-[46px] shrink-0 overflow-hidden rounded-[8px] border border-border-subtle bg-layer-1">
      {url ? <img src={url} alt="" className="size-full object-contain" /> : <div className="size-full animate-pulse bg-layer-2" />}
      <span className="absolute inset-x-0 bottom-0 truncate bg-layer-2/90 px-0.5 text-center text-[8px] leading-[13px] font-semibold">{image.zoomOf ? `${name} ⌕` : name}</span>
      <button type="button" aria-label={`Remove ${label} from this message`} onClick={onRemove} className="absolute top-0 right-0 grid size-5 place-items-center text-fg-tertiary">
        <X className="size-3" />
      </button>
    </li>
  )
}

export type TrayProps = {
  board: Board
  plan: SendPlan
  /** Marks off screen are included (the opt-in). */
  offScreen: boolean
  onOffScreen(on: boolean): void
  onRemove(index: number): void
  compose(i: PlannedImage): Promise<Blob>
  /** The model cannot see images and no vision fallback is set. */
  noVision: boolean
  /** The agent's one-line status, a notice while full-window, or an approval waiting. */
  status: string | null
}

/** Above the box: the session picker and the agent's status line, then what the send carries besides the words. */
function Tray({ pick, ...p }: TrayProps & { pick: SessionPick }) {
  const marks = p.plan.marks.length
  const carries = p.plan.images.length > 0 || p.plan.offScreen > 0
  return (
    <div className="mb-1.5 flex flex-col gap-1.5">
      <div className="flex min-w-0 items-center gap-1.5">
        <SessionChip p={pick} />
        {p.status && (
          <p className="wb-glass min-w-0 flex-1 truncate rounded-full px-2.5 py-1.5 text-[12px]" aria-live="polite" title={p.status}>
            {p.status}
          </p>
        )}
      </div>
      {carries && (
        <div className="wb-glass flex flex-col gap-1 rounded-2xl p-1.5">
          <ul className="flex items-center gap-1.5 overflow-x-auto" aria-label="What this message carries">
            {p.plan.images.map((image, i) => (
              <Thumb key={imageKey(image)} image={image} board={p.board} compose={p.compose} onRemove={() => p.onRemove(i)} />
            ))}
            {p.plan.offScreen > 0 && (
              <li className="shrink-0">
                <label className="flex items-center gap-1.5 px-1 text-[12px] text-fg-secondary">
                  <input type="checkbox" checked={p.offScreen} onChange={(e) => p.onOffScreen(e.target.checked)} />+ {p.plan.offScreen} {p.plan.offScreen === 1 ? 'mark' : 'marks'} off screen
                </label>
              </li>
            )}
          </ul>
          <p className="px-0.5 text-[11px] text-fg-tertiary" aria-live="polite">
            {p.noVision
              ? 'This model cannot see marks — captions only.'
              : `${p.plan.images.length} ${p.plan.images.length === 1 ? 'image' : 'images'} · ${marks} ${marks === 1 ? 'mark' : 'marks'}`}
          </p>
        </div>
      )}
    </div>
  )
}

export type WhiteboardComposerProps = {
  doc: string
  pick: SessionPick
  tools: Omit<ClusterProps, 'onHold'>
  tray: TrayProps
  deliver(m: { text: string; files: string[]; clientId: string }): Promise<string>
  /** The send went (the session it went to; a first send's new one). */
  onSent(session: string): void
  dockRef?: Ref<HTMLDivElement>
}

export function WhiteboardComposer(p: WhiteboardComposerProps) {
  return (
    <div
      ref={p.dockRef}
      role="group"
      aria-label="Composer"
      data-whiteboard-composer=""
      className="wb-dock absolute z-30 [&_[data-composer]]:shadow-lg"
      onPointerDown={(e) => e.stopPropagation()}
    >
      <Composer
        key={p.pick.session ?? 'new'}
        session={p.pick.session}
        draftScope={`wb:${p.doc}:`}
        compact
        ownModel
        lead={<ToolsMenu {...p.tools} />}
        strip={<Tray {...p.tray} pick={p.pick} />}
        carries={p.tray.plan.marks.length > 0}
        deliver={p.deliver}
        onSent={p.onSent}
      />
    </div>
  )
}
