import { Link } from '@tanstack/react-router'
import { ChevronLeft } from 'lucide-react'
import { type ComponentType, type ReactNode, type Ref } from 'react'
import { Editor as BlockEditor } from '@/components/editor/Editor'
import type { LiveDocument } from '@/components/editor/useDocument'
import { SectionBand } from '@/components/nav/SectionBand'
import { SectionRow } from '@/components/nav/SectionRow'
import { ParticipantStrip } from '@/components/share/ParticipantStrip'
import { ShareAction } from '@/components/share/ShareDialog'
import { SearchTrigger } from '@/components/sidebar/SearchTrigger'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { useIsMobile } from '@/hooks/use-mobile'
import { isMacDesktop } from '@/lib/hotkeys'
import { groupOf } from '@/lib/nav'
import { isPopout } from '@/lib/popout'
import { cn } from 'cn'

/**
 * The bars over a document pane's body. In the shell (97b): the section's `SectionBand` (unless the page's explorer
 * already renders it, `band={false}`), then a `SectionRow` with the phone-only back chevron, `title` and `actions`.
 * In a popped window: the 52 px drag bar of 49 §5.1 (back chevron and search trigger under `md`), unchanged.
 * `title` is the caller's own node (an `h2`, a `code`, a title input) and sits in the row as-is.
 * 118: the back chevron hides from `md` up because Notes/Knowledge have their list pane beside the document there;
 * a page without one (a task) passes `backAlways` and keeps its back at every width, in both shells.
 */
export function Frame({
  backTo,
  backLabel,
  backAlways = false,
  title,
  actions,
  band = true,
  children,
}: {
  backTo: '/knowledge' | '/notes' | '/tasks' | '/agents'
  backLabel: string
  backAlways?: boolean
  title: ReactNode
  actions?: ReactNode
  band?: boolean
  children: ReactNode
}) {
  const backClass = backAlways ? undefined : 'md:hidden'
  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      {isPopout() ? (
        <header
          data-tauri-drag-region={isMacDesktop() || undefined}
          className="flex h-[52px] shrink-0 items-center gap-1 border-b border-border-subtle px-3"
        >
          <Button asChild variant="ghost" size="icon-sm" className={backClass} aria-label={backLabel}>
            <Link to={backTo}>
              <ChevronLeft />
            </Link>
          </Button>
          {title}
          {actions}
          <SearchTrigger className="md:hidden" />
        </header>
      ) : (
        <>
          {band && <SectionBand group={groupOf(backTo) ?? 'library'} />}
          <SectionRow back={{ to: backTo, label: backLabel, className: backClass }}>
            {title}
            {actions}
          </SectionRow>
        </>
      )}
      {children}
    </article>
  )
}

/** One icon action in the frame's bar. */
export function Action({
  label,
  icon: Icon,
  onClick,
  pressed,
  disabled,
}: {
  label: string
  icon: ComponentType<{ className?: string }>
  onClick: () => void
  pressed?: boolean
  disabled?: boolean
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          className="size-11 rounded-xl"
          aria-label={label}
          aria-pressed={pressed}
          disabled={disabled}
          onClick={onClick}
        >
          <Icon className={cn('size-[18px]', (pressed === false || disabled) && 'text-fg-tertiary')} />
        </Button>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  )
}

/** The live document in BlockNote (59 §7.1: knowledge is created from its file on first open). `dense`: the tighter paper of the meeting pane. */
export function Live({ provider, status, error, onAsk, dense }: LiveDocument & { onAsk?: (block: string) => void; dense?: boolean }) {
  if (error) {
    return <p className="m-auto max-w-full px-6 text-center text-sm text-danger wrap-anywhere">{error.message}</p>
  }
  if (!provider) {
    return (
      <div className="mx-auto w-full max-w-[768px] space-y-3 p-6">
        <Skeleton className="h-8 w-1/2" />
        <Skeleton className="h-4 w-3/4" />
        <Skeleton className="h-4 w-2/3" />
      </div>
    )
  }
  return <BlockEditor provider={provider} status={status} onAsk={onAsk} dense={dense} />
}

/**
 * The live page of one document (59 §7.3): `Frame` + `Live`, with the owner's extras around the editor —
 * `above` (a search return), `rail` (beside the body; under `md` below it), `below` (a note's chat panel),
 * and any dialogs/sheets as `children`. `fallback` replaces `Live` while the document cannot be shown
 * (59 §24.y: the knowledge page's read-only markdown view).
 */
export function DocumentPage({
  live,
  fallback,
  onAsk,
  columnRef,
  above,
  rail,
  below,
  children,
  actions,
  ...frame
}: {
  live: LiveDocument
  fallback?: ReactNode
  onAsk?: (block: string) => void
  /** The editor column, for `useSearchFocus` (49 §7). */
  columnRef?: Ref<HTMLDivElement>
  above?: ReactNode
  rail?: ReactNode
  below?: ReactNode
  children?: ReactNode
  backTo: '/knowledge' | '/notes'
  backLabel: string
  title: ReactNode
  actions?: ReactNode
  band?: boolean
}) {
  const mobile = useIsMobile()
  // 59 §8.2, §8.5: who is here, and the owner's "Share…" — every document page alike.
  const bar = live.provider ? (
    <>
      <ParticipantStrip provider={live.provider} className="mr-2 hidden md:flex" />
      {actions}
      <ShareAction provider={live.provider} />
    </>
  ) : (
    actions
  )
  return (
    <Frame {...frame} actions={bar}>
      <div className="flex min-h-0 flex-1">
        <div ref={columnRef} tabIndex={-1} className="flex min-w-0 flex-1 flex-col outline-none">
          {above}
          {fallback ?? <Live {...live} onAsk={onAsk} />}
          {mobile && rail && (
            <section className="max-h-[40%] shrink-0 overflow-y-auto border-t border-border-subtle">{rail}</section>
          )}
        </div>
        {!mobile && rail && (
          <aside className="w-[260px] shrink-0 overflow-y-auto border-l border-border-subtle">{rail}</aside>
        )}
      </div>
      {below}
      {children}
    </Frame>
  )
}
