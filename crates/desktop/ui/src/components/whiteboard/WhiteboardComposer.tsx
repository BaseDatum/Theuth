// 218 composer: adapter over the shared `FloatingComposerDock` (dictation, voice notes, attachments, model and effort,
// skills, send/stop, keys, paste), docked bottom right on the whiteboard at iPhone width. Adds the whiteboard tools
// button (`ToolsMenu`) and the planned-image/marks tray above the box.

import { X } from 'lucide-react'
import { type Ref, type RefObject, useEffect, useState } from 'react'
import { FloatingComposerDock } from '@/components/chat/floating/FloatingComposerDock'
import type { ReplyBubbleProps } from '@/components/chat/floating/ReplyBubble'
import { SessionChip as FloatingSessionChip, shortName, useSessionTitle } from '@/components/chat/floating/SessionChip'
import type { PlannedImage, SendPlan } from './canvas/api'
import type { Board } from './canvas/model'
import { type ClusterProps, ToolsMenu } from './Cluster'
import { imageKey } from './send/send'
import type { WhiteboardSession } from './wire'

export { shortName, useSessionTitle }

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

/** The session picker: New session, this whiteboard's sessions, recent others, detach. */
export function SessionChip({ p }: { p: SessionPick }) {
  return (
    <FloatingSessionChip
      session={p.session}
      title={p.title}
      bound={p.bound}
      boundLabel="On this whiteboard"
      busy={p.busy}
      onPick={p.onPick}
      onNew={p.onNew}
      onDetach={p.onDetach}
      onOpen={p.onOpen}
      canvasSelector=".wb-canvas"
    />
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
  /** Optional status override (e.g. a notice while full-window). */
  status?: string | null
}

/** What the send carries besides the words: thumbnails per planned image + off-screen marks opt-in. */
function MarksTray(p: TrayProps) {
  const marks = p.plan.marks.length
  const carries = p.plan.images.length > 0 || p.plan.offScreen > 0
  if (!carries) return null
  return (
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
  containerRef?: RefObject<HTMLElement | null>
  reply?: Omit<ReplyBubbleProps, 'floating' | 'beside'> | null
  beside?: boolean
  hideOverlays?: boolean
}

export function WhiteboardComposer(p: WhiteboardComposerProps) {
  return (
    <FloatingComposerDock
      session={p.pick.session}
      draftScope={`wb:${p.doc}:`}
      sessionChip={{
        title: p.pick.title,
        bound: p.pick.bound,
        boundLabel: 'On this whiteboard',
        onPick: p.pick.onPick,
        onNew: p.pick.onNew,
        onDetach: p.pick.onDetach,
        onOpen: p.pick.onOpen,
        canvasSelector: '.wb-canvas',
      }}
      statusOverride={p.tray.status}
      tray={<MarksTray {...p.tray} />}
      lead={<ToolsMenu {...p.tools} />}
      carries={p.tray.plan.marks.length > 0}
      deliver={p.deliver}
      onSent={p.onSent}
      dockRef={p.dockRef}
      containerRef={p.containerRef}
      reply={p.reply}
      beside={p.beside}
      hideOverlays={p.hideOverlays}
      marker="whiteboard"
    />
  )
}
