import { useNavigate } from '@tanstack/react-router'
import { AppWindow, BookOpen, Copy, ExternalLink, FileText, type LucideIcon, MessageSquare, Mic, Paperclip, StickyNote, X } from 'lucide-react'
import type { DragEvent } from 'react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { type AttachRef, DRAG_TYPE, KIND_LABEL, type Kind, attachTo } from '@/lib/attach'
import { touch } from '@/lib/platform'
import { type PopoutTarget, canPopout, openPopout } from '@/lib/popout'
import { cn } from '@/lib/utils'

/** §7.5 icons; an artifact chip has no record, so it takes the generic file glyph (picker rows refine by artifact kind). */
export const KIND_ICON: Record<Kind, LucideIcon> = {
  note: StickyNote,
  knowledge: BookOpen,
  artifact: FileText,
  session: MessageSquare,
  meeting: Mic,
}

/** Props that make a row a drag source for the composer's drop (§7.8); desktop only — no HTML5 drag on iOS Safari. */
export function dragSource(ref: () => AttachRef): { draggable: boolean; onDragStart(e: DragEvent): void } {
  return {
    draggable: !touch(),
    onDragStart(e) {
      e.dataTransfer.setData(DRAG_TYPE, JSON.stringify(ref()))
      e.dataTransfer.effectAllowed = 'copy'
    },
  }
}

/** Navigate to the thing a reference points at (§7.6); a knowledge `id@commit` opens 49's historical mode. */
export function useOpenRef(): (ref: AttachRef) => void {
  const navigate = useNavigate()
  return (ref) => {
    // 144 §12.4/§8: `notebook` and `source` refs exist before `Kind` lists them (the contract worker widens `lib/attach`).
    const kind: string = ref.kind
    if (kind === 'notebook') return void navigate({ to: '/notebooks/$id', params: { id: ref.id }, search: {} })
    if (kind === 'source') return void navigate({ to: '/sources/$id', params: { id: ref.id }, search: {} })
    switch (ref.kind) {
      case 'note':
        return void navigate({ to: '/notes/$id', params: { id: ref.id }, search: {} })
      case 'knowledge': {
        const at = ref.id.indexOf('@')
        return void navigate({
          to: '/knowledge/$',
          params: { _splat: at < 0 ? ref.id : ref.id.slice(0, at) },
          search: at < 0 ? {} : { change: ref.id.slice(at + 1) },
        })
      }
      case 'artifact':
        return void navigate({ to: '/artifacts/$callId', params: { callId: ref.id } })
      case 'session':
        return void navigate({ to: '/c/$session', params: { session: ref.id }, search: {} })
      case 'meeting':
        return void navigate({ to: '/meetings/$id', params: { id: ref.id } })
    }
  }
}

/** The window a reference can open alone (`lib/popout`), `null` for knowledge (no popped route). */
export function popoutOf(ref: AttachRef): PopoutTarget | null {
  switch (ref.kind) {
    case 'note':
      return { kind: 'note', doc: ref.id }
    case 'artifact':
      return { kind: 'artifact', callId: ref.id }
    case 'session':
      return { kind: 'chat', session: ref.id }
    case 'meeting':
      return { kind: 'meeting', meeting: ref.id }
    case 'knowledge':
      return null
  }
}

/**
 * One attached reference as a pill: kind icon, title, muted kind. Click opens a small popover with the subtitle,
 * **Open** and — where the container can and the kind has a popped route — **Pop out**; `again` (a history chip,
 * §7.9) adds **Attach again** (a chip in `again`'s composer) and **Copy id**; `onRemove` puts a × on the pill.
 */
export function AttachChip({ item: ref, again, onRemove, className }: { item: AttachRef; again?: string | null; onRemove?: () => void; className?: string }) {
  const popout = popoutOf(ref)
  const open = useOpenRef()
  const Icon = KIND_ICON[ref.kind] ?? Paperclip
  const label = KIND_LABEL[ref.kind] ?? ref.kind
  return (
    <Popover>
      <PopoverTrigger asChild>
        <button
          type="button"
          className={cn(
            'flex h-7 max-w-[240px] items-center gap-1 rounded-full bg-layer-2 px-2 text-xs hover:bg-layer-3 pointer-coarse:h-8',
            className,
          )}
          aria-label={`${label}: ${ref.title}`}
        >
          <Icon className="size-3.5 shrink-0 text-fg-secondary" />
          <span className="truncate">{ref.title}</span>
          <span className="shrink-0 text-fg-tertiary">{label}</span>
          {onRemove && (
            <span
              role="button"
              tabIndex={0}
              aria-label={`Remove ${ref.title}`}
              className="-mr-1 ml-0.5 flex size-5 shrink-0 items-center justify-center rounded-full text-fg-tertiary hover:bg-layer-3 hover:text-fg pointer-coarse:size-6"
              onClick={(e) => {
                e.stopPropagation()
                onRemove()
              }}
              onKeyDown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                  e.preventDefault()
                  e.stopPropagation()
                  onRemove()
                }
              }}
            >
              <X className="size-3" />
            </span>
          )}
        </button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-auto max-w-80 p-2 text-sm">
        <div className="px-1 pb-1">
          <div className="font-medium break-words">{ref.title}</div>
          {ref.meta && <div className="text-xs text-fg-secondary break-words">{ref.meta}</div>}
        </div>
        <div className="flex flex-wrap gap-1">
          <Button variant="ghost" size="sm" onClick={() => open(ref)}>
            <ExternalLink /> Open
          </Button>
          {popout && canPopout() && (
            <Button variant="ghost" size="sm" onClick={() => void openPopout(popout)}>
              <AppWindow /> Pop out
            </Button>
          )}
          {again !== undefined && (
            <Button variant="ghost" size="sm" onClick={() => attachTo(again, ref)}>
              <Paperclip /> Attach again
            </Button>
          )}
          {again !== undefined && (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => navigator.clipboard.writeText(ref.id).then(() => toast('Copied'), () => toast.error('Copy failed'))}
            >
              <Copy /> Copy id
            </Button>
          )}
        </div>
      </PopoverContent>
    </Popover>
  )
}
