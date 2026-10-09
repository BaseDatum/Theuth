import { Link } from '@tanstack/react-router'
import { ChevronDown, ChevronLeft } from 'lucide-react'
import { type ComponentType, type ReactNode, type Ref } from 'react'
import { Editor as BlockEditor } from '@/components/editor/Editor'
import type { LiveDocument } from '@/components/editor/useDocument'
import { SectionBand } from '@/components/nav/SectionBand'
import { SectionRow } from '@/components/nav/SectionRow'
import { useItemNotebooks, useNotebook } from '@/components/notebooks/useNotebook'
import { ParticipantStrip } from '@/components/share/ParticipantStrip'
import { ShareAction } from '@/components/share/ShareDialog'
import { SearchTrigger } from '@/components/sidebar/SearchTrigger'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { Skeleton } from '@/components/ui/skeleton'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { useIsMobile } from '@/hooks/use-mobile'
import { isMacDesktop } from '@/lib/hotkeys'
import { groupOf } from '@/lib/nav'
import { isPopout } from '@/lib/popout'
import type { DocKind } from '@/rpc/bindings/DocKind'
import type { ItemKind } from '@/rpc/bindings/ItemKind'
import { cn } from 'cn'

/**
 * 144 §4 (Sky's ruling, 2026-10-08): the way back from a note or source to the notebook it was opened from — chevron
 * + "Back to <title>" at EVERY width, at the left of the `SectionRow`, in place of the phone-only list chevron.
 * `search.tab` lands the phone on the right column; the desktop restores the three columns.
 */
export type NotebookBack = { to: '/notebooks/$id'; params: { id: string }; search: { tab: 'notes' | 'sources' }; label: string; always: true }

/**
 * 156 D1: a meeting note belongs to its meeting — `manifest.note` points at it and the meeting's own Delete removes it —
 * so the note page never offers Delete for one. `meeting` is the page's own knowledge (the meeting pane, the popped
 * meeting note) before `meta` lands; `kind` covers `/notes/<id>` opened straight on one.
 */
export const ownedByMeeting = (kind: DocKind | undefined, meeting: string | undefined): boolean => kind === 'meeting_note' || meeting !== undefined

/**
 * The bars over a document pane's body. In the shell (97b): the section's `SectionBand` (unless the page's explorer
 * already renders it, `band={false}`), then a `SectionRow` with the phone-only back chevron, `title` and `actions`.
 * In a popped window: the 52 px drag bar of 49 §5.1 (back chevron and search trigger under `md`), unchanged.
 * `title` is the caller's own node (an `h2`, a `code`, a title input) and sits in the row as-is.
 * 118: the back chevron hides from `md` up because Notes/Knowledge have their list pane beside the document there;
 * a page without one (a task) passes `backAlways` and keeps its back at every width, in both shells.
 * 144 §4: `back` (a `NotebookBack`) replaces the row's list chevron with the always-visible notebook control; the
 * popped window keeps 143's rule (no band, no back) and ignores it.
 * 156 D1: `embedded` (the meeting page's note pane) draws neither the band nor any back link — the `SectionRow` alone
 * is the pane's toolbar, with `title` and `actions`.
 */
export function Frame({
  backTo,
  backLabel,
  backAlways = false,
  back,
  title,
  actions,
  band = true,
  embedded = false,
  children,
}: {
  backTo: '/knowledge' | '/notes' | '/notebooks' | '/tasks' | '/agents'
  backLabel: string
  backAlways?: boolean
  back?: NotebookBack
  title: ReactNode
  actions?: ReactNode
  band?: boolean
  embedded?: boolean
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
      ) : embedded ? (
        // A pane can be narrow: the actions wrap under the title instead of squeezing it to nothing.
        <SectionRow className="h-auto min-h-11 flex-wrap py-1">
          {title}
          {actions}
        </SectionRow>
      ) : (
        <>
          {band && <SectionBand group={groupOf(backTo) ?? 'library'} />}
          <SectionRow back={back ? undefined : { to: backTo, label: backLabel, className: backClass }}>
            {back && (
              <Button asChild variant="ghost" size="sm" className="-ml-1 shrink-0 px-1.5" aria-label={back.label} data-testid="notebook-back">
                <Link to={back.to} params={back.params} search={back.search}>
                  <ChevronLeft />
                  <span className="hidden max-w-[220px] truncate md:inline">{back.label}</span>
                </Link>
              </Button>
            )}
            {title}
            {actions}
          </SectionRow>
        </>
      )}
      {children}
    </article>
  )
}

/**
 * 144 §4: the back control for a note (`tab: 'notes'`) or source (`tab: 'sources'`) page. With `?nb` the control
 * goes there ("Back to <title>", "Back to notebook" while the row loads). Without it, an item that belongs to
 * notebooks goes back to its first one, and `chip` is the `in <N notebooks> ▾` menu listing all of them (the way
 * back exists from Library too). A popped window gets neither (143's rule).
 */
export function useNotebookBack(kind: ItemKind, item: string, nb: string | undefined): { back: NotebookBack | undefined; chip: ReactNode } {
  const tab = kind === 'note' ? 'notes' : 'sources'
  const member = useItemNotebooks(kind, item)
  const target = nb ?? member?.[0]?.doc.id
  const row = useNotebook(target ?? '', target !== undefined).data
  if (isPopout() || target === undefined) return { back: undefined, chip: null }
  const title = row?.doc.title
  const back: NotebookBack = { to: '/notebooks/$id', params: { id: target }, search: { tab }, label: title ? `Back to ${title}` : 'Back to notebook', always: true }
  const chip =
    !nb && member && member.length > 0 ? (
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="outline" size="xs" className="shrink-0 rounded-full text-[11px] font-normal text-fg-tertiary" aria-label={`In ${member.length} ${member.length === 1 ? 'notebook' : 'notebooks'}`}>
            in {member.length} {member.length === 1 ? 'notebook' : 'notebooks'} <ChevronDown />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start">
          {member.map((n) => (
            <DropdownMenuItem key={n.doc.id} asChild>
              <Link to="/notebooks/$id" params={{ id: n.doc.id }} search={{ tab }}>
                {n.doc.title || 'Untitled'}
              </Link>
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
    ) : null
  return { back, chip }
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
  backTo: '/knowledge' | '/notes' | '/notebooks'
  backLabel: string
  title: ReactNode
  actions?: ReactNode
  band?: boolean
  embedded?: boolean
  back?: NotebookBack
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
