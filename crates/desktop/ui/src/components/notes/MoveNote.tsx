import { useMutation } from '@tanstack/react-query'
import { MoveRight } from 'lucide-react'
import { notify as toast } from '@/lib/notify'
import { type Mesh, inMesh } from '@/components/notes/machine'
import { useServerView } from '@/components/notes/ServerChip'
import { errorText } from '@/components/settings/shared'
import { engines } from '@/components/tasks/home'
import { age } from '@/components/tasks/helpers'
import { useMesh } from '@/components/tasks/queries'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import type { EngineProvider } from '@/lib/doc/provider'
import { useApi } from '@/lib/rpc'
import type { Api } from '@/rpc/bindings/client'
import { cn } from '@/lib/utils'
import type { DocKind } from "@/rpc/bindings/method.agent.output"

/** 184 §12.4: no item for other kinds (a task thread moves with its task), the reason an item is disabled, or null = it may move. */
export function moveRefusal(kind: DocKind | undefined, homesOn: boolean): 'hide' | string | null {
  if (kind === 'board') return 'Boards stay on the machine whose folder they point at (root:).'
  if (kind === 'meeting_note') return 'Meeting notes stay on the machine that recorded them.'
  if (kind !== 'note') return 'hide'
  return homesOn ? null : 'Notes can move once every machine runs this version.'
}

export type MoveRow = { id: string; name: string; online: boolean; enabled: boolean; detail: string }

/** One row per machine; `home` lives here, an unreachable one is disabled (`offline 5m`), and while a move runs (`pending` = its target) every row is. */
export function moveRows(mesh: Mesh, home: string | null, pending: string | null = null, now = Date.now()): MoveRow[] {
  return engines(mesh).map((e) => {
    const detail = e.id === pending ? 'Moving…' : e.id === home ? 'lives here' : e.online ? '' : e.seen > 0 ? `offline ${age(e.seen, now)}` : 'offline'
    return { id: e.id, name: e.name, online: e.online, enabled: e.online && e.id !== home && pending === null, detail }
  })
}

/** `doc_move_home`: toasts `Moved to <machine>`, or the engine's refusal verbatim. */
export const moveOptions = (api: Pick<Api, 'doc_move_home'>, doc: string) => ({
  mutationFn: (to: { id: string; name: string }) => api.doc_move_home(doc, to.id),
  onSuccess: (_: unknown, to: { name: string }) => toast(`Moved to ${to.name}`),
  onError: (e: unknown) => toast.error(errorText(e)),
})

/** 184 §12.4: Move to ▸ machine in the note bar. In a mesh only; an unhomed note's current home is its server (the holder). */
export function MoveNote({ provider }: { provider: EngineProvider }) {
  const api = useApi()
  const mesh = useMesh()
  useServerView(provider) // re-render on `Server` events / re-opens: the current home of an unhomed note is its server
  const move = useMutation(moveOptions(api, provider.id))
  const refusal = moveRefusal(provider.meta?.kind, !!mesh.data?.docs_homes_on)
  if (!inMesh(mesh.data) || refusal === 'hide') return null
  const rows = moveRows(mesh.data, provider.meta?.home ?? provider.server, move.isPending ? (move.variables?.id ?? null) : null)
  return (
    <DropdownMenu onOpenChange={(open) => open && void mesh.refetch()}>
      <span title={refusal ?? undefined}>
        <DropdownMenuTrigger asChild>
          <Button variant="ghost" className="size-11 rounded-xl" aria-label="Move to" disabled={refusal !== null} aria-describedby={refusal ? 'move-refusal' : undefined}>
            <MoveRight className="size-[18px]" />
          </Button>
        </DropdownMenuTrigger>
        {refusal && (
          <span id="move-refusal" className="sr-only">
            {refusal}
          </span>
        )}
      </span>
      <DropdownMenuContent align="end" className="w-64">
        {rows.map((r) => (
          <DropdownMenuItem key={r.id} disabled={!r.enabled} onSelect={() => move.mutate({ id: r.id, name: r.name })} className="flex-col items-start gap-0">
            <span className="flex items-center gap-1.5">
              <span className={cn('size-1.5 rounded-full', r.online ? 'bg-success' : 'bg-fg-tertiary')} />
              {r.name}
            </span>
            {r.detail && <span className="pl-3 text-[11px] text-fg-tertiary">{r.detail}</span>}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
