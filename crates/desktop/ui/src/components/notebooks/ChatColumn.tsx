import { useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { ChevronDown, ExternalLink, SquarePen } from 'lucide-react'
import { useEffect } from 'react'
import { toast } from 'sonner'
import { create } from 'zustand'
import { Composer } from '@/components/chat/Composer'
import { Transcript } from '@/components/chat/Transcript'
import { NotebookContext } from '@/components/notebooks/NotebookContext'
import { useNotebook, useNotebookChats } from '@/components/notebooks/useNotebook'
import { ago } from '@/components/sidebar/AgentTree'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import type { AttachRef } from '@/lib/attach'
import { notebookKeys, useNotebooksApi } from '@/lib/notebooks-rpc'
import { queryKeys, useApi } from '@/lib/rpc'
import type { DocSummary } from '@/rpc/bindings/DocSummary'
import { useSessions } from '@/store/sessions'

// 144 §8: the server lists a notebook's chats; this device remembers only which thread is open per notebook.
const KEY = 'notebook-chat'
type Saved = Record<string, string | null | undefined>

function load(): Saved {
  try {
    return JSON.parse(globalThis.localStorage?.getItem(KEY) ?? '{}') as Saved
  } catch {
    return {}
  }
}

export const useOpenThread = create<{ open: Saved; set(notebook: string, session: string | null): void }>()((set, get) => ({
  open: load(),
  set(notebook, session) {
    const open = { ...get().open, [notebook]: session }
    localStorage.setItem(KEY, JSON.stringify(open))
    set({ open })
  },
}))

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
 */
export function ChatColumn({ notebook, title, notes, composerOnly = false }: { notebook: string; title: string; notes: readonly DocSummary[]; composerOnly?: boolean }) {
  const nb = useNotebooksApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const remembered = useOpenThread((s) => s.open[notebook])
  const remember = useOpenThread((s) => s.set)
  const chats = useNotebookChats(notebook).data ?? []
  // Only a notebook never opened here adopts the newest thread; an explicit "New chat" (`null`) stays new.
  const session = remembered === undefined ? (chats[0]?.id ?? null) : remembered
  useLoadSession(session)
  const row = useNotebook(notebook).data
  const sources = row?.sources ?? 0
  const pinned: AttachRef = { kind: 'notebook', id: notebook, title, meta: `${sources} sources · ${notes.length} notes` }
  const onChanged = () => {
    void qc.invalidateQueries({ queryKey: notebookKeys.items(notebook) })
    void qc.invalidateQueries({ queryKey: queryKeys.notes })
  }

  const composer = (
    <div className="shrink-0 border-t border-border-subtle bg-background">
      <div className="flex items-center gap-2 px-3 pt-2 text-[11px] text-fg-tertiary">
        <span>
          in context: <b className="font-semibold text-fg-secondary">{sources} sources</b> · <b className="font-semibold text-fg-secondary">{notes.length} notes</b>
        </span>
        <span className="h-1 flex-1 rounded-sm bg-layer-2" title="wave 2" aria-hidden>
          <span className="block h-full w-0 rounded-sm bg-primary" />
        </span>
      </div>
      <Composer
        key={session ?? 'new'}
        session={session}
        pinned={pinned}
        sessionTitle={title}
        onCreated={(id) => {
          remember(notebook, id)
          nb.session_set_notebook(id, notebook).then(
            () => void qc.invalidateQueries({ queryKey: notebookKeys.chats(notebook) }),
            (e: Error) => toast.error(e.message),
          )
        }}
        compact
      />
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
