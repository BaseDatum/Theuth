import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { MoreHorizontal } from 'lucide-react'
import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Input } from '@/components/ui/input'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { usePanes } from '@/lib/panes'
import { queryKeys, useApi } from '@/lib/rpc'
import type { CompactStyle } from '@/rpc/bindings/CompactStyle'
import { useSessionField, useSessions } from '@/store/sessions'
import { type Ctx, openSurface, run, settings, type Surface, surfaceOf } from './slash'

const STYLES: CompactStyle[] = ['summary', 'cut', 'snap']

/** Rename a session; `rename_session` fires `TitleChanged`, which updates the slice and the list. */
export function RenameDialog({
  session,
  open,
  onOpenChange,
}: {
  session: string
  open: boolean
  onOpenChange(o: boolean): void
}) {
  const api = useApi()
  const qc = useQueryClient()
  const title = useSessionField(session, 'title')
  const [value, setValue] = useState('')
  useEffect(() => {
    if (open) setValue(title ?? '')
  }, [open, title])
  const save = async () => {
    onOpenChange(false)
    try {
      await api.rename_session(session, value.trim())
      void qc.invalidateQueries({ queryKey: queryKeys.sessions })
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-sm">
        <DialogHeader>
          <DialogTitle>Rename chat</DialogTitle>
        </DialogHeader>
        <Input
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && value.trim() && void save()}
          autoFocus
        />
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button disabled={!value.trim()} onClick={() => void save()}>
            Save
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function PromptSheet({
  session,
  open,
  onOpenChange,
}: {
  session: string
  open: boolean
  onOpenChange(o: boolean): void
}) {
  const api = useApi()
  const { data, error } = useQuery({
    queryKey: queryKeys.prompt(session),
    queryFn: () => api.get_prompt(session),
    enabled: open,
  })
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent className="w-full sm:w-[640px] sm:max-w-[640px]">
        <SheetHeader>
          <SheetTitle>System prompt</SheetTitle>
          <SheetDescription>What the model sees at the top of this session.</SheetDescription>
        </SheetHeader>
        <pre className="mx-4 mb-4 min-h-0 flex-1 overflow-auto rounded-xl bg-layer-2 p-4 font-mono text-xs leading-5 whitespace-pre-wrap">
          {error ? (error instanceof Error ? error.message : String(error)) : (data ?? 'Loading…')}
        </pre>
      </SheetContent>
    </Sheet>
  )
}

function JobsSheet({
  session,
  open,
  onOpenChange,
}: {
  session: string
  open: boolean
  onOpenChange(o: boolean): void
}) {
  const api = useApi()
  const key = queryKeys.jobs(session)
  const { data: jobs = [], refetch } = useQuery({
    queryKey: key,
    queryFn: () => api.list_jobs(session),
    enabled: open,
    refetchInterval: open ? 2000 : false,
  })
  const kill = async (id: string) => {
    try {
      await api.cancel_job(session, id)
      toast(`killed ${id}`)
      void refetch()
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent className="w-full sm:w-[640px] sm:max-w-[640px]">
        <SheetHeader>
          <SheetTitle>Background jobs</SheetTitle>
          <SheetDescription>
            {jobs.length === 0 ? 'No jobs in this session.' : `${jobs.length} in this session`}
          </SheetDescription>
        </SheetHeader>
        <div className="mx-4 mb-4 min-h-0 flex-1 overflow-auto text-sm">
          <table className="w-full">
            <thead className="text-left text-xs text-fg-tertiary">
              <tr>
                <th className="py-1 font-medium">id</th>
                <th className="py-1 font-medium">command</th>
                <th className="py-1 font-medium">state</th>
                <th className="py-1 font-medium">secs</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {jobs.map((j) => (
                <tr key={j.id} className="border-t border-border-subtle align-top">
                  <td className="py-2 font-mono text-xs">{j.id}</td>
                  <td className="max-w-64 truncate py-2 font-mono text-xs" title={j.command}>
                    {j.name ? `${j.name} · ` : ''}
                    {j.command}
                  </td>
                  <td className="py-2">{j.state}</td>
                  <td className="py-2 tabular-nums">{Number(j.secs)}</td>
                  <td className="py-1 text-right">
                    {j.state === 'running' && (
                      <Button size="xs" variant="outline" onClick={() => void kill(j.id)}>
                        Kill
                      </Button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </SheetContent>
    </Sheet>
  )
}

/** The `…` menu of the open session; every action is the slash command the TUI would run. */
export function SessionMenu({ session }: { session: string }) {
  const api = useApi()
  const navigate = useNavigate()
  const parent = useSessionField(session, 'parent')
  const undone = useSessionField(session, 'undone') ?? 0
  const [rename, setRename] = useState(false)
  const [sheet, setSheet] = useState<Surface | null>(null)

  const focused = usePanes((s) => s.focused === session)
  useEffect(() => {
    if (!focused) return
    const on = (e: Event) => {
      const s = surfaceOf(e)
      if (s === 'prompt' || s === 'jobs') setSheet(s)
    }
    window.addEventListener('theuth:open', on)
    return () => window.removeEventListener('theuth:open', on)
  }, [focused])

  const ctx: Ctx = {
    api,
    session,
    parent,
    navigate,
    async reload() {
      useSessions
        .getState()
        .loadHistory(session, await api.load_session(session, null, 60), 'replace')
    },
  }
  const cmd = (line: string) => () =>
    void run(line, ctx).catch((e: unknown) =>
      toast.error(e instanceof Error ? e.message : String(e)),
    )

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            className="size-9 rounded-lg text-fg-secondary"
            aria-label="Session menu"
          >
            <MoreHorizontal className="size-5" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-56">
          <DropdownMenuItem onSelect={() => setRename(true)}>Rename</DropdownMenuItem>
          <DropdownMenuItem onSelect={() => void navigator.clipboard.writeText(session)}>
            Copy id
          </DropdownMenuItem>
          {parent && <DropdownMenuItem onSelect={cmd('/parent')}>Open parent</DropdownMenuItem>}
          <DropdownMenuSeparator />
          <DropdownMenuSub>
            <DropdownMenuSubTrigger>Compact</DropdownMenuSubTrigger>
            <DropdownMenuSubContent>
              {STYLES.map((s) => (
                <DropdownMenuItem key={s} onSelect={cmd(`/compact ${s}`)}>
                  {s}
                </DropdownMenuItem>
              ))}
            </DropdownMenuSubContent>
          </DropdownMenuSub>
          <DropdownMenuItem onSelect={cmd('/clear')}>Clear</DropdownMenuItem>
          <DropdownMenuItem onSelect={cmd('/shake')}>Shake context…</DropdownMenuItem>
          <DropdownMenuItem onSelect={cmd('/undo')}>Undo</DropdownMenuItem>
          <DropdownMenuItem disabled={undone === 0} onSelect={cmd('/redo')}>
            Redo
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem onSelect={cmd('/knowledge harvest')}>
            Harvest knowledge
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={() => openSurface('prompt')}>Show prompt</DropdownMenuItem>
          <DropdownMenuItem onSelect={() => openSurface('jobs')}>Jobs</DropdownMenuItem>
          <DropdownMenuItem onSelect={() => void settings(navigate, 'communication', session)}>
            Communication &amp; peers…
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <RenameDialog session={session} open={rename} onOpenChange={setRename} />
      <PromptSheet
        session={session}
        open={sheet === 'prompt'}
        onOpenChange={(o) => !o && setSheet(null)}
      />
      <JobsSheet
        session={session}
        open={sheet === 'jobs'}
        onOpenChange={(o) => !o && setSheet(null)}
      />
    </>
  )
}
