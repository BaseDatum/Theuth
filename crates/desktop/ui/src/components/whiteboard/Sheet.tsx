// 218 §8.8 "Expanded sheet" / design §4.3: the pill grown upward. A text field (Scribble and dictation work in it), the
// tray — one thumbnail per image the send will carry, each removable, "+ N marks off screen" as an opt-in — the session
// chip, the model picker of the bound session (A2), Send, and a chevron that opens the session in chat.

import { ArrowUp, ChevronUp, X } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { ComposerModel } from '@/components/chat/ModelMenu'
import type { PlannedImage, SendPlan } from './canvas/api'
import type { Board } from './canvas/model'
import { type SessionPick, SessionChip } from './Pill'

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
    <li className="relative h-[84px] w-[62px] shrink-0 overflow-hidden rounded-[9px] border border-border-subtle bg-layer-1">
      {url ? <img src={url} alt="" className="size-full object-contain" /> : <div className="size-full animate-pulse bg-layer-2" />}
      <span className="absolute inset-x-0 bottom-0 truncate bg-layer-2/90 px-1 text-center text-[8.5px] leading-[15px] font-semibold">{image.zoomOf ? `${name} ⌕` : name}</span>
      <button type="button" aria-label={`Remove ${label} from this message`} onClick={onRemove} className="absolute top-0 right-0 grid size-6 place-items-center text-fg-tertiary">
        <X className="size-3.5" />
      </button>
    </li>
  )
}

export type SheetProps = {
  pick: SessionPick
  board: Board
  plan: SendPlan
  /** Marks off screen are included (the opt-in). */
  offScreen: boolean
  onOffScreen(on: boolean): void
  onRemove(index: number): void
  compose(i: PlannedImage): Promise<Blob>
  /** The model cannot see images and no vision fallback is set. */
  noVision: boolean
  sending: boolean
  text: string
  onText(t: string): void
  onSend(): void
  onClose(): void
  onOpenSession(): void
  /** Focus the field on open (mic, ⌘↵, pill tap). */
  autoFocus: boolean
}

export function Sheet(p: SheetProps) {
  const field = useRef<HTMLTextAreaElement>(null)
  useEffect(() => {
    if (p.autoFocus) field.current?.focus()
  }, [p.autoFocus])
  const marks = p.plan.marks.length
  const canSend = !p.sending && (p.text.trim().length > 0 || marks > 0)
  return (
    <div
      role="dialog"
      aria-label="Message the agent about this whiteboard"
      className="wb-glass wb-corner wb-rise absolute z-30 flex w-[min(380px,calc(100vw-48px))] flex-col gap-2 rounded-[24px] p-3.5"
      onKeyDown={(e) => {
        if (e.key === 'Escape') {
          e.stopPropagation()
          p.onClose()
        } else if (e.key === 'Enter' && (e.metaKey || e.ctrlKey) && canSend) {
          e.preventDefault()
          p.onSend()
        }
      }}
    >
      <div className="flex items-center justify-between gap-2">
        <SessionChip p={p.pick} />
        <div className="flex min-w-0 items-center gap-1">
          {p.pick.session && <ComposerModel session={p.pick.session} own />}
          <button type="button" aria-label="Collapse" onClick={p.onClose} className="wb-press grid size-8 place-items-center rounded-full">
            <X className="size-4" />
          </button>
        </div>
      </div>
      {(p.plan.images.length > 0 || p.plan.offScreen > 0) && (
        <div className="flex flex-col gap-1">
          <ul className="flex items-center gap-2 overflow-x-auto pb-0.5" aria-label="What this message carries">
            {p.plan.images.map((image, i) => (
              <Thumb key={`${image.card ?? 'canvas'}:${image.zoomOf ?? ''}:${image.marks.join(',')}`} image={image} board={p.board} compose={p.compose} onRemove={() => p.onRemove(i)} />
            ))}
            {p.plan.offScreen > 0 && (
              <li className="shrink-0">
                <label className="wb-muted flex items-center gap-1.5 px-1 text-[12px]">
                  <input type="checkbox" checked={p.offScreen} onChange={(e) => p.onOffScreen(e.target.checked)} />+ {p.plan.offScreen} {p.plan.offScreen === 1 ? 'mark' : 'marks'} off screen
                </label>
              </li>
            )}
          </ul>
          <p className="wb-muted text-[12px]" aria-live="polite">
            {p.noVision
              ? 'This model cannot see marks — captions only.'
              : `${p.plan.images.length} ${p.plan.images.length === 1 ? 'image' : 'images'} · ${marks} ${marks === 1 ? 'mark' : 'marks'}`}
          </p>
        </div>
      )}
      <textarea
        ref={field}
        value={p.text}
        onChange={(e) => p.onText(e.target.value)}
        placeholder={marks > 0 ? 'What do the marks mean?' : 'Ask about this whiteboard…'}
        aria-label="Message"
        rows={3}
        className="min-h-[66px] w-full resize-none bg-transparent text-[14.5px] leading-[1.45] outline-none placeholder:text-current placeholder:opacity-55"
      />
      <div className="flex items-center justify-between gap-2">
        <button type="button" onClick={p.onOpenSession} disabled={!p.pick.session} className="wb-press wb-muted flex h-8 items-center gap-1 rounded-full px-2 text-[12px] disabled:opacity-40">
          <ChevronUp className="size-4" aria-hidden />
          Open session
        </button>
        <button
          type="button"
          onClick={p.onSend}
          disabled={!canSend}
          className="flex h-9 items-center gap-1.5 rounded-full bg-accent px-4 text-[13px] font-semibold text-white disabled:opacity-45"
        >
          {p.pick.session ? (p.pick.busy ? 'Steer' : 'Send') : 'Send to a new session'}
          <ArrowUp className="size-4" aria-hidden />
        </button>
      </div>
    </div>
  )
}
