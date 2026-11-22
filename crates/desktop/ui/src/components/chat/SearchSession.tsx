import { useNavigate } from '@tanstack/react-router'
import { ArrowDown } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { SearchReturn } from '@/components/sidebar/SearchReturn'
import { SearchTrigger } from '@/components/sidebar/SearchTrigger'
import { Button } from '@/components/ui/button'
import { useSidebar } from '@/components/ui/sidebar'
import { isMacDesktop } from '@/lib/hotkeys'
import { useApi } from '@/lib/rpc'
import { type SearchDestination, isSearchDestination, useSearchOrigin } from '@/lib/search'
import { cn } from '@/lib/utils'
import { fromHistoryPage } from '@/store/reduce'
import { type Entry, useSessionField } from '@/store/sessions'
import { ArtifactPanel } from './ArtifactPanel'
import { SidebarToggle } from './SidebarToggle'
import { Message } from './Message'

const PAGE = 60

type View =
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | { status: 'ready'; entries: Entry[]; more: boolean; undone: number }

/** Whether the card's stored row is message `seq` (49 §7): the row's `context.target`, never a local index — one row can become several cards. */
function isTarget(e: Entry, seq: number): boolean {
  const t = e.context?.target
  return t?.kind === 'message' && t.seq === seq
}

/**
 * Earlier messages inspection of one session (49 §7): the history page ending at message `seq`
 * (`before = seq + 1`), rendered read-only and kept apart from the canonical live slice, so events
 * never stitch across the gap to the tail or move the target. No composer: sending from an
 * incomplete view would not be the model's context. Jump to latest returns to the ordinary chat.
 */
export function SearchSession({ session, seq }: { session: string; seq: number }) {
  const api = useApi()
  const navigate = useNavigate()
  const origin = useSearchOrigin()
  const title = useSessionField(session, 'title')
  const { open, isMobile } = useSidebar()
  const mac = isMacDesktop()
  const destination = useMemo<SearchDestination>(() => ({ category: 'sessions', session, seq }), [session, seq])
  const [view, setView] = useState<View>({ status: 'loading' })
  // Bumped when the same hit is activated again while already shown: the page is re-read and the target re-revealed.
  const [gen, setGen] = useState(0)
  const seen = useRef(origin)
  useEffect(() => {
    if (seen.current === origin) return
    seen.current = origin
    if (isSearchDestination(origin, destination)) setGen((g) => g + 1)
  }, [origin, destination])

  useEffect(() => {
    document.title = `Earlier messages · ${title ?? session}`
  }, [title, session])

  // Fenced: a reply landing after the route (or engine) moved on is dropped, never shown for another target.
  useEffect(() => {
    let live = true
    setView({ status: 'loading' })
    api.load_session(session, seq + 1, PAGE).then(
      (page) => {
        if (!live) return
        setView({ status: 'ready', entries: fromHistoryPage(page.history), more: page.more, undone: page.undone })
      },
      (e: unknown) => live && setView({ status: 'error', message: e instanceof Error ? e.message : String(e) }),
    )
    return () => {
      live = false
    }
  }, [api, session, seq, gen])

  // Reveal: scroll the first target card into view and focus it, once the page rendered.
  const scroller = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (view.status !== 'ready') return
    const el = scroller.current?.querySelector<HTMLElement>('[data-search-target]')
    el?.scrollIntoView({ block: 'center' })
    el?.focus({ preventScroll: true })
  }, [view])

  const jump = () => void navigate({ to: '/c/$session', params: { session }, search: {} })
  const missing = view.status === 'ready' && !view.entries.some((e) => isTarget(e, seq))
  // ponytail: plain list, no virtualizer; one page is at most 60 rows plus one seq group. Virtualize if pages grow.
  return (
    <>
      <header data-tauri-drag-region={mac || undefined} className={cn('flex h-[52px] shrink-0 items-center gap-2 px-3', !open && mac && 'pl-20')}>
        {(!open || isMobile) && (
          <div className="flex shrink-0 items-center">
            <SearchTrigger />
            <SidebarToggle />
          </div>
        )}
        <div className="min-w-0">
          <div className="truncate text-sm font-medium">Earlier messages</div>
          {title && <div className="truncate text-xs text-fg-secondary">{title}</div>}
        </div>
        <div className="flex-1" data-tauri-drag-region={mac || undefined} />
        <SearchReturn />
        <Button variant="outline" size="sm" onClick={jump}>
          <ArrowDown />
          Jump to latest
        </Button>
        <ArtifactPanel session={session} />
      </header>
      <div ref={scroller} className="min-h-0 flex-1 overflow-x-hidden overflow-y-auto wrap-anywhere">
        <div className="mx-auto w-full md:max-w-[768px] px-4 pb-8">
          <p role="status" className="my-3 rounded-lg bg-layer-1 px-3 py-2 text-xs text-fg-secondary">
            Read-only view of the messages up to message {seq}. New activity in this session is not shown here; Jump to latest opens the live chat.
            {view.status === 'ready' && view.more && ' Older messages exist before this page.'}
          </p>
          {view.status === 'loading' && <p className="py-6 text-center text-sm text-fg-tertiary">Loading…</p>}
          {view.status === 'error' && (
            <div role="alert" className="my-3 rounded-lg bg-danger-bg px-3 py-2 text-sm text-danger">
              Could not load this page: {view.message}
            </div>
          )}
          {view.status === 'ready' && missing && (
            <div role="alert" className="my-3 rounded-lg border border-warning/40 px-3 py-2 text-sm">
              Message {seq} is not in this session's visible history: it was removed, undone
              {view.undone > 0 && ` (${view.undone} undone ${view.undone === 1 ? 'turn hides' : 'turns hide'} newer messages)`} or never stored. Nothing below is highlighted.
            </div>
          )}
          {view.status === 'ready' &&
            view.entries.map((entry) => {
              const target = isTarget(entry, seq)
              return (
                <div
                  key={entry.id}
                  data-search-target={target || undefined}
                  tabIndex={target ? -1 : undefined}
                  aria-label={target ? `Search result: message ${seq}` : undefined}
                  className={cn(target && 'my-1 rounded-2xl px-2 ring-2 ring-fg-accent/40 outline-none')}
                >
                  {target && <div className="pt-2 text-xs font-medium text-fg-accent">Search result · message {seq}</div>}
                  <Message entry={entry} prompts={null} session={session} readOnly reveal={target} />
                </div>
              )
            })}
        </div>
      </div>
    </>
  )
}
