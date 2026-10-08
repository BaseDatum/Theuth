// 144 §12.1: `+ Add ▾` on the Sources column — File… / URL / Meeting… / Paste / Chat… / Confluence / Jira. YouTube and
// audio/video are detected (the engine routes them; the UI only words the toast).
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { ChevronDown, Plus } from 'lucide-react'
import { type ReactNode, useRef, useState } from 'react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { extFor, putAttachment } from '@/lib/files'
import { type SourceSpec, notebookKeys, useNotebooksApi } from '@/lib/notebooks-rpc'
import { queryKeys, useApi } from '@/lib/rpc'
import { formatAt } from '@/lib/time'
import { detectFile, detectUrl, sourceTitle } from './origin'

type Which = 'url' | 'meeting' | 'paste' | 'chat' | 'confluence' | 'jira'

/** `source_add` payload; `lead` is a toast shown while the engine works (`Adding YouTube video…`), `as` words the success. */
type Add = { spec: SourceSpec; title: string | null; lead?: string; as?: string }

export function AddSourceMenu({ notebook }: { notebook: string }) {
  const api = useApi()
  const nb = useNotebooksApi()
  const qc = useQueryClient()
  const [open, setOpen] = useState<Which | null>(null)
  const file = useRef<HTMLInputElement>(null)
  const add = useMutation({
    mutationFn: ({ spec, title, lead }: Add) => {
      if (lead) toast(lead)
      return nb.source_add(notebook, spec, title)
    },
    onSuccess: (s, v) => {
      toast(`Added ${v.as ? `${v.as} ` : ''}“${sourceTitle(s)}”`)
      void qc.invalidateQueries({ queryKey: notebookKeys.sources(notebook) })
      void qc.invalidateQueries({ queryKey: notebookKeys.one(notebook) })
      setOpen(null)
    },
    onError: (e) => toast.error(e.message),
  })
  const pickFile = async (f: File) => {
    try {
      const path = await putAttachment(api, f, extFor(f))
      const kind = detectFile(f.name)
      add.mutate({ spec: { path }, title: f.name, as: kind === 'file' ? undefined : kind })
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }
  const dialog = { open, onClose: () => setOpen(null), pending: add.isPending }
  return (
    <>
      <input
        ref={file}
        type="file"
        className="hidden"
        onChange={(e) => {
          const f = e.target.files?.[0]
          e.target.value = ''
          if (f) void pickFile(f)
        }}
      />
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="outline" size="sm" disabled={add.isPending}>
            <Plus /> Add <ChevronDown />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuItem onSelect={() => file.current?.click()}>File…</DropdownMenuItem>
          <DropdownMenuItem onSelect={() => setOpen('url')}>URL</DropdownMenuItem>
          <DropdownMenuItem onSelect={() => setOpen('meeting')}>Meeting…</DropdownMenuItem>
          <DropdownMenuItem onSelect={() => setOpen('paste')}>Paste</DropdownMenuItem>
          <DropdownMenuItem onSelect={() => setOpen('chat')}>Chat…</DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem onSelect={() => setOpen('confluence')}>Confluence</DropdownMenuItem>
          <DropdownMenuItem onSelect={() => setOpen('jira')}>Jira</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <UrlDialog {...dialog} onAdd={(a) => add.mutate(a)} />
      <PasteDialog {...dialog} onAdd={(a) => add.mutate(a)} />
      <McpDialog {...dialog} onAdd={(a) => add.mutate(a)} />
      {open === 'meeting' && <MeetingDialog {...dialog} onAdd={(a) => add.mutate(a)} />}
      {open === 'chat' && <ChatDialog {...dialog} onAdd={(a) => add.mutate(a)} />}
    </>
  )
}

type DialogProps = { open: Which | null; onClose: () => void; pending: boolean; onAdd: (a: Add) => void }

/** One small form dialog: `onSubmit` runs when `ready`. */
function Form({ title, description, shown, onClose, ready, onSubmit, pending, children }: { title: string; description: string; shown: boolean; onClose: () => void; ready: boolean; onSubmit: () => void; pending: boolean; children: ReactNode }) {
  return (
    <Dialog open={shown} onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="sm:max-w-[520px]">
        <form
          className="flex flex-col gap-4"
          onSubmit={(e) => {
            e.preventDefault()
            if (ready && !pending) onSubmit()
          }}
        >
          <DialogHeader>
            <DialogTitle>{title}</DialogTitle>
            <DialogDescription>{description}</DialogDescription>
          </DialogHeader>
          {children}
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit" disabled={!ready || pending}>
              Add
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

function UrlDialog({ open, onClose, pending, onAdd }: DialogProps) {
  const [url, setUrl] = useState('')
  const v = url.trim()
  return (
    <Form title="Add a URL" description="A web page, or a YouTube link (its transcript is fetched)." shown={open === 'url'} onClose={onClose} ready={v !== ''} pending={pending}
      onSubmit={() => onAdd({ spec: { url: v }, title: null, lead: detectUrl(v) === 'youtube' ? 'Adding YouTube video…' : undefined })}>
      <Input value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://…" autoFocus spellCheck={false} />
    </Form>
  )
}

function PasteDialog({ open, onClose, pending, onAdd }: DialogProps) {
  const [text, setText] = useState('')
  const [title, setTitle] = useState('')
  return (
    <Form title="Paste text" description="Kept as a source; the title is optional." shown={open === 'paste'} onClose={onClose} ready={text.trim() !== ''} pending={pending}
      onSubmit={() => onAdd({ spec: { text }, title: title.trim() || null })}>
      <Input value={title} onChange={(e) => setTitle(e.target.value)} placeholder="Title (optional)" />
      <Textarea value={text} onChange={(e) => setText(e.target.value)} placeholder="Paste here" className="min-h-[200px]" autoFocus />
    </Form>
  )
}

/** Confluence/Jira go through the engine's MCP servers; the `server`/`tool` names are the engine's defaults. */
const MCP: Record<'confluence' | 'jira', { title: string; description: string; placeholder: string; server: string; tool: string }> = {
  confluence: { title: 'Add a Confluence page', description: 'A page URL or id, read through the confluence MCP server.', placeholder: 'Page URL or id', server: 'confluence', tool: 'get_page' },
  jira: { title: 'Add a Jira issue', description: 'An issue key, read through the jira MCP server.', placeholder: 'DR-412', server: 'jira', tool: 'get_issue' },
}

function McpDialog({ open, onClose, pending, onAdd }: DialogProps) {
  const [value, setValue] = useState('')
  const which = open === 'confluence' || open === 'jira' ? open : null
  const m = MCP[which ?? 'jira']
  const v = value.trim()
  return (
    <Form title={m.title} description={m.description} shown={which !== null} onClose={onClose} ready={v !== ''} pending={pending}
      onSubmit={() => onAdd({ spec: { mcp: { server: m.server, tool: m.tool, args: { id: v } } }, title: null })}>
      <Input value={value} onChange={(e) => setValue(e.target.value)} placeholder={m.placeholder} autoFocus spellCheck={false} />
    </Form>
  )
}

/** A pick list: rows of `{ id, label, sub }`; a click adds. */
function PickDialog({ title, description, rows, onClose, pending, onPick }: { title: string; description: string; rows: { id: string; label: string; sub: string }[] | undefined; onClose: () => void; pending: boolean; onPick: (id: string, label: string) => void }) {
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="sm:max-w-[520px]">
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>{description}</DialogDescription>
        </DialogHeader>
        <ul className="-mx-2 max-h-[60vh] overflow-y-auto">
          {rows === undefined ? (
            <li className="px-2 py-1.5 text-sm text-fg-tertiary">Loading…</li>
          ) : rows.length === 0 ? (
            <li className="px-2 py-1.5 text-sm text-fg-tertiary">Nothing to add.</li>
          ) : (
            rows.map((r) => (
              <li key={r.id}>
                <button type="button" disabled={pending} className="flex w-full flex-col items-start rounded-md px-2 py-1.5 text-left hover:bg-layer-2 disabled:opacity-50" onClick={() => onPick(r.id, r.label)}>
                  <span className="w-full truncate text-sm">{r.label}</span>
                  <span className="text-xs text-fg-tertiary">{r.sub}</span>
                </button>
              </li>
            ))
          )}
        </ul>
      </DialogContent>
    </Dialog>
  )
}

function MeetingDialog({ onClose, pending, onAdd }: DialogProps) {
  const api = useApi()
  const q = useQuery({ queryKey: queryKeys.meetings, queryFn: () => api.meeting_list(null) })
  const rows = q.data?.map((m) => ({ id: m.id, label: m.title ?? m.process_name, sub: formatAt(Date.parse(m.started_at)) }))
  return <PickDialog title="Add a meeting" description="Its transcript and summary become one source." rows={rows} onClose={onClose} pending={pending} onPick={(id) => onAdd({ spec: { meeting: id }, title: null })} />
}

function ChatDialog({ onClose, pending, onAdd }: DialogProps) {
  const api = useApi()
  const q = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() })
  const rows = q.data
    ?.filter((s) => !s.parent && !s.desk)
    .sort((a, b) => b.updated_at.localeCompare(a.updated_at))
    .slice(0, 50)
    .map((s) => ({ id: s.id, label: s.title || 'Untitled chat', sub: formatAt(Date.parse(s.updated_at)) }))
  return <PickDialog title="Add a past chat" description="The newest 50 chats; the transcript becomes one source." rows={rows} onClose={onClose} pending={pending} onPick={(id) => onAdd({ spec: { session: id }, title: null })} />
}
