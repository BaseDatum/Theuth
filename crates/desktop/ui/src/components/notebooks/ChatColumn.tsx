import { useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { ChevronDown, ExternalLink, SquarePen } from 'lucide-react'
import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { Composer, PLAIN_DRAFT } from '@/components/chat/Composer'
import { MemoryBadge, MemoryPanel } from '@/components/chat/MemoryPanel'
import { Transcript } from '@/components/chat/Transcript'
import { NotebookContext, applyContextDefaults, useEndlessSettings, useNotebookSession, useOpenThread } from '@/components/notebooks/NotebookContext'
import { useContextEstimate, useNotebook, useNotebookChats } from '@/components/notebooks/useNotebook'
import { ago } from '@/components/sidebar/AgentTree'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { Switch } from '@/components/ui/switch'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import type { AttachRef } from '@/lib/attach'
import { useCompactorLabel } from '@/lib/endless'
import { contextLine } from '@/lib/notebook-context'
import { notebookKeys, useContextApi, useNotebooksApi } from '@/lib/notebooks-rpc'
import { queryKeys, useApi } from '@/lib/rpc'
import type { ContextEstimate } from "@/rpc/bindings/method.agent.output"
import type { DocSummary } from "@/rpc/bindings/method.agent.output"
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
 * 157 D1–D3 / 215 C3: the composer carries the model picker (`ComposerModel` via `ownModel`, the thread's or the draft's
 * pick; `/model` opens it, `own`), the header the ∞ switch for a new chat (NewChatPane's rule: `agent.endless.default_chat`, remembered per draft, the
 * compactor line under the composer while on; applied as `memory: 'endless'` on both creation paths) and, for an
 * existing endless thread, the ∞ badge that opens the Memory panel. `/endless on|off` switches an existing thread.
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
  const summary = session ? chats.find((c) => c.id === session) : undefined
  const [memory, setMemory] = useState(false)
  // 157 D3: the new chat's ∞ choice — the draft's own once touched, else the engine default; forgotten with the draft.
  const settings = useEndlessSettings()
  const picked = useOpenThread((s) => s.endless[notebook])
  const endless = picked ?? settings?.default_chat ?? false
  const setEndless = useOpenThread((s) => s.setEndless)
  const compactor = useCompactorLabel(settings?.model)
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
      // that turn ran on the default rule unless a dial was touched first (then `useEnsureNotebookSession` created
      // the thread with its rows before the send and this path never runs).
      await applyContextDefaults(ctx, id, notebook)
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
    void qc.invalidateQueries({ queryKey: notebookKeys.chats(notebook) })
    void qc.invalidateQueries({ queryKey: ['notebooks', 'estimate', notebook] })
  }

  const composer = (
    <div className="shrink-0 border-t border-border-subtle bg-background">
      <ContextLine estimate={estimate.data} error={estimate.error} />
      {!session && endless && <p className="px-3 pt-1 text-[11px] text-fg-tertiary">Fractal context · compactor: {compactor}</p>}
      <Composer key={session ?? 'new'} session={session} pinned={pinned} sessionTitle={title} draftCreate={{ ...PLAIN_DRAFT, endless }} onCreated={(id) => void onCreated(id)} compact ownModel />
    </div>
  )
  if (composerOnly) return composer

  return (
    <section className="@container flex min-h-0 min-w-0 flex-1 flex-col" aria-label="Chat about this notebook">
      <header className={HEADER}>
        {/* A narrow column (the notes pane dragged wide) drops the label and the New chat text first. */}
        <span className="hidden shrink-0 whitespace-nowrap @lg:inline">
          Chat <span className="font-normal text-fg-tertiary">· this notebook</span>
        </span>
        <div className="min-w-0 flex-1" />
        {session ? (
          <MemoryBadge session={session} summary={summary} open={memory} onToggle={() => setMemory((o) => !o)} />
        ) : (
          <Tooltip>
            <TooltipTrigger asChild>
              <label className="flex shrink-0 items-center gap-1.5">
                <Switch checked={endless} onCheckedChange={(on) => setEndless(notebook, on)} aria-label="Fractal context" />
                <span aria-hidden className="relative -top-px text-sm leading-4 [font-feature-settings:normal]">
                  {'\u221E'}
                </span>
              </label>
            </TooltipTrigger>
            <TooltipContent side="bottom">Fractal context for this new chat{endless ? ` · compactor: ${compactor}` : ''}</TooltipContent>
          </Tooltip>
        )}
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="outline" size="sm">
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
        <Button size="sm" variant="ghost" aria-label="New chat" title="New chat" onClick={() => remember(notebook, null)}>
          <SquarePen /> <span className="hidden @md:inline">New chat</span>
        </Button>
        {session && (
          <Button variant="ghost" size="icon-sm" aria-label="Open in the chat list" title="Open in the chat list" onClick={() => void navigate({ to: '/c/$session', params: { session }, search: {} })}>
            <ExternalLink />
          </Button>
        )}
      </header>
      {session && <MemoryPanel session={session} open={memory} onOpenChange={setMemory} />}
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
 * §5.2 / 157 D9: `in context: 4 sources (2 summary · 1 full · 1 mention) · 2 notes · ~12.4k tokens · 1 omitted`, the
 * `N omitted` chip that scrolls the column to the first omitted card (`id="item-<id>"`) and, once the block is in the
 * chat, `2 changes go with your next message`. No budget bar: the first injection is sized by the chat's own context.
 * Wraps to two lines on the phone.
 */
export function ContextLine({ estimate, error }: { estimate: ContextEstimate | undefined; error: Error | null }) {
  if (error) return <p className="px-3 pt-2 text-[11px] text-danger">{error.message}</p>
  if (!estimate) return <p className="px-3 pt-2 text-[11px] text-fg-tertiary">in context: …</p>
  const parts = contextLine(estimate)
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
    </div>
  )
}
