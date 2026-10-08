import { useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { ChevronDown, ExternalLink, SquarePen } from 'lucide-react'
import { useEffect } from 'react'
import { toast } from 'sonner'
import { Composer } from '@/components/chat/Composer'
import { Transcript } from '@/components/chat/Transcript'
import { NotebookContext, useNotebookSession, useOpenThread } from '@/components/notebooks/NotebookContext'
import { useContextEstimate, useNotebook, useNotebookChats } from '@/components/notebooks/useNotebook'
import { ago } from '@/components/sidebar/AgentTree'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import type { AttachRef } from '@/lib/attach'
import { contextLine, remembered, useContextDefaults } from '@/lib/notebook-context'
import { notebookKeys, useContextApi, useNotebooksApi } from '@/lib/notebooks-rpc'
import { queryKeys, useApi } from '@/lib/rpc'
import type { ContextEstimate } from '@/rpc/bindings/ContextEstimate'
import { cn } from '@/lib/utils'
import type { DocSummary } from '@/rpc/bindings/DocSummary'
import { useSessions } from '@/store/sessions'

const PAGE = 60

/** The thread's session page, once: what `c.$session.tsx` does on first load, without its todo/agents extras (`NoteChat.tsx`). */
function useLoadSession(session: string | null) {
  const api = useApi()
  useEffect(() => {
    if (!session) return
    const st = useSessions.getState()
    st.ensure(session)
    if (st.sessions[session]?.loaded) return
    api.load_session(session, null, PAGE).then(
      (page) => useSessions.getState().loadHistory(session, page, 'replace'),
      (e: unknown) => toast.error(e instanceof Error ? e.message : String(e)),
    )
  }, [api, session])
}

const HEADER = 'flex h-10 shrink-0 items-center gap-2 border-b border-border-subtle px-3 text-xs font-semibold'

/**
 * 144 §12.1 Chat: the notebook's centre column. `Chats · N ▾` lists the notebook's sessions (newest first); the open
 * one is remembered per device; **New chat** starts another thread in the same notebook. Every message leads with the
 * notebook's `[attached notebook …]` marker; the first message's session is bound with `session_set_notebook`.
 * `composerOnly`: the phone's Sources tab pins just the context line and the composer.
 * Wave 2 §5.2: the context line under the header comes from `notebook_context_estimate`; a new thread gets this
 * device's remembered dial positions (`notebook_context_set` per item) right after its session exists.
 */
export function ChatColumn({ notebook, title, notes, composerOnly = false }: { notebook: string; title: string; notes: readonly DocSummary[]; composerOnly?: boolean }) {
  const nb = useNotebooksApi()
  const ctx = useContextApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const remember = useOpenThread((s) => s.set)
  const chats = useNotebookChats(notebook).data ?? []
  const session = useNotebookSession(notebook)
  useLoadSession(session)
  const row = useNotebook(notebook).data
  const sources = row?.sources ?? 0
  const estimate = useContextEstimate(notebook, session)
  const pinned: AttachRef = { kind: 'notebook', id: notebook, title, meta: `${sources} sources · ${notes.length} notes` }
  const onChanged = () => {
    void qc.invalidateQueries({ queryKey: notebookKeys.items(notebook) })
    void qc.invalidateQueries({ queryKey: queryKeys.notes })
  }
  const onCreated = async (id: string) => {
    remember(notebook, id)
    try {
      await nb.session_set_notebook(id, notebook)
      // §5.1 W-g: the device defaults become this thread's rows. The composer has already sent the first message, so
      // the first turn may still run on the default rule (the same seam wave 1's `session_set_notebook` has).
      await Promise.all(remembered(useContextDefaults.getState().defaults, notebook).map((r) => ctx.notebook_context_set(id, r.item, r.kind, r.level)))
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
    void qc.invalidateQueries({ queryKey: notebookKeys.chats(notebook) })
    void qc.invalidateQueries({ queryKey: ['notebooks', 'estimate', notebook] })
  }

  const composer = (
    <div className="shrink-0 border-t border-border-subtle bg-background">
      <ContextLine estimate={estimate.data} error={estimate.error} />
      <Composer key={session ?? 'new'} session={session} pinned={pinned} sessionTitle={title} onCreated={(id) => void onCreated(id)} compact />
    </div>
  )
  if (composerOnly) return composer

  return (
    <section className="flex min-h-0 min-w-0 flex-1 flex-col" aria-label="Chat about this notebook">
      <header className={HEADER}>
        <span className="min-w-0 truncate whitespace-nowrap">
          Chat <span className="font-normal text-fg-tertiary">· this notebook</span>
        </span>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="outline" size="sm" className="ml-auto">
              Chats · {chats.length} <ChevronDown />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            {chats.length === 0 && <DropdownMenuItem disabled>No chats yet</DropdownMenuItem>}
            {chats.map((c) => (
              <DropdownMenuItem key={c.id} onSelect={() => remember(notebook, c.id)}>
                <span className="min-w-0 flex-1 truncate">{c.title || 'Untitled'}</span>
                <span className="text-xs text-fg-tertiary">{ago(c.updated_at)}</span>
              </DropdownMenuItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
        <Button size="sm" variant="ghost" onClick={() => remember(notebook, null)}>
          <SquarePen /> New chat
        </Button>
        {session && (
          <Button variant="ghost" size="icon-sm" aria-label="Open in the chat list" title="Open in the chat list" onClick={() => void navigate({ to: '/c/$session', params: { session }, search: {} })}>
            <ExternalLink />
          </Button>
        )}
      </header>
      <NotebookContext value={{ notebook, notes, onChanged }}>
        {session ? (
          <Transcript key={session} session={session} compact />
        ) : (
          <div className="flex flex-1 items-center justify-center p-6 text-center text-sm text-fg-tertiary">
            Ask theuth about this notebook {'\u2014'} the first message starts a chat with it attached
          </div>
        )}
        {composer}
      </NotebookContext>
    </section>
  )
}

/**
 * §5.2: `in context: 4 sources (2 summary · 1 full · 1 mention) · 2 notes · ~12.4k of 40k tokens · 1 omitted`, a thin
 * budget bar (amber from 80 %, red when something was truncated) and the `N omitted` chip that scrolls the column to
 * the first omitted card (`id="item-<id>"`). Wraps to two lines on the phone.
 */
export function ContextLine({ estimate, error }: { estimate: ContextEstimate | undefined; error: Error | null }) {
  if (error) return <p className="px-3 pt-2 text-[11px] text-danger">{error.message}</p>
  if (!estimate) return <p className="px-3 pt-2 text-[11px] text-fg-tertiary">in context: …</p>
  const parts = contextLine(estimate)
  const ratio = estimate.budget_tokens > 0 ? Math.min(1, estimate.total_tokens / estimate.budget_tokens) : 0
  const truncated = estimate.items.some((i) => i.status === 'truncated')
  const firstOmitted = estimate.items.find((i) => i.status === 'omitted')
  const omittedLabel = `${estimate.omitted} omitted`
  return (
    <div className="flex flex-wrap items-center gap-x-2 gap-y-1 px-3 pt-2 text-[11px] text-fg-tertiary" data-testid="context-line">
      <span className="min-w-0">
        {estimate.empty ? (
          <span className="text-amber-700">{parts[0]}</span>
        ) : (
          <>
            in context:{' '}
            {parts
              .filter((p) => p !== omittedLabel)
              .map((p, i) => (
                <span key={p}>
                  {i > 0 && ' · '}
                  <b className="font-semibold text-fg-secondary">{p}</b>
                </span>
              ))}
          </>
        )}
      </span>
      {estimate.omitted > 0 && (
        <button
          type="button"
          className="rounded-full border border-border-subtle px-1.5 leading-4 hover:bg-layer-2"
          title="Scroll to the first omitted item"
          onClick={() => firstOmitted && document.getElementById(`item-${firstOmitted.id}`)?.scrollIntoView({ block: 'center', behavior: 'smooth' })}
        >
          {omittedLabel}
        </button>
      )}
      <span
        role="progressbar"
        aria-label="Context budget"
        aria-valuemin={0}
        aria-valuemax={estimate.budget_tokens}
        aria-valuenow={estimate.total_tokens}
        className="h-1 min-w-16 flex-1 basis-full rounded-sm bg-layer-2 md:basis-auto"
      >
        <span className={cn('block h-full rounded-sm', truncated ? 'bg-danger' : ratio >= 0.8 ? 'bg-amber-500' : 'bg-primary')} style={{ width: `${Math.round(ratio * 100)}%` }} />
      </span>
    </div>
  )
}
