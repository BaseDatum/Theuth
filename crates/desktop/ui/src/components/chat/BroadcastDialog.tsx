import { Capability, useQuerySupport } from '@/rpc/capabilities'
import { useQuery } from '@tanstack/react-query'
import { useEffect, useMemo, useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { Desks } from '@/components/sidebar/Desks'
import { SessionList } from '@/components/sidebar/SessionList'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { SidebarMenu } from '@/components/ui/sidebar'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { useIsMobile } from '@/hooks/use-mobile'
import { deskList, deskSessions } from '@/lib/desks'
import { queryKeys, useApi } from '@/lib/rpc'
import type { Ticks } from '@/lib/selection'
import { useAgents } from '@/components/tasks/queries'
import { cn } from '@/lib/utils'
import type { BroadcastReceipt } from "@/rpc/bindings/method.agent.output"
import type { Delivery } from "@/rpc/bindings/method.agent.output"
import type { AgentInfo } from '@/store/ui-events'
import { Composer } from './Composer'
import { allRoots, BROADCAST_DRAFT, type BroadcastRequest, deskRoots, outcomeLines, runningRoots, summarize } from './broadcast'

const DESCRIPTION = 'One message to many sessions; each recipient sees it marked as a broadcast.'

/**
 * Mounted once beside the sidebar. `Dialog` on desktop, bottom sheet on phones (the sidebar's own split). `active` is
 * the sidebar's open chat: the list below marks it and shows its agent tree as the sidebar does.
 */
export function BroadcastDialog({ active }: { active?: string }) {
  const [req, setReq] = useState<BroadcastRequest | null>(null)
  const isMobile = useIsMobile()
  useEffect(() => {
    const on = (e: Event) => setReq((e as CustomEvent<BroadcastRequest>).detail)
    window.addEventListener('theuth:broadcast', on)
    return () => window.removeEventListener('theuth:broadcast', on)
  }, [])
  const close = () => setReq(null)
  if (isMobile) {
    return (
      <Sheet open={req !== null} onOpenChange={(o) => !o && close()}>
        <SheetContent side="bottom" className="flex max-h-[90vh] flex-col gap-2 px-4 pb-[calc(1rem+var(--safe-bottom))]">
          <SheetHeader className="px-0">
            <SheetTitle>Broadcast</SheetTitle>
            <SheetDescription>{DESCRIPTION}</SheetDescription>
          </SheetHeader>
          {req && <Form req={req} active={active} onDone={close} />}
        </SheetContent>
      </Sheet>
    )
  }
  return (
    <Dialog open={req !== null} onOpenChange={(o) => !o && close()}>
      <DialogContent className="flex max-h-[85vh] flex-col sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>Broadcast</DialogTitle>
          <DialogDescription>{DESCRIPTION}</DialogDescription>
        </DialogHeader>
        {req && <Form req={req} active={active} onDone={close} />}
      </DialogContent>
    </Dialog>
  )
}

type Preview = { receipt: BroadcastReceipt } | { error: string }

/**
 * The new chat's `Composer` (its send — button or Enter — broadcasts to the ticked sessions), then delivery, the
 * `dry_run: true` preview and the preset chips; below them, only the targets list scrolls: the sidebar's own Desks
 * and chat list (`Ticks`), every session row a checkbox.
 */
export function Form({ req, active, onDone }: { req: BroadcastRequest; active?: string; onDone(): void }) {
  const canQuery1 = useQuerySupport(["list_activity"])
  const canQuery1_2 = useQuerySupport(["list_sessions"])
  const api = useApi()
  const { data: rows, error: loadError } = useQuery({ enabled: canQuery1, queryKey: ['activity'], queryFn: () => api.list_activity() })
  const { data: sessions } = useQuery({ enabled: canQuery1_2, queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() })
  // The sidebar's desks, exactly: each active roster agent's current desk session (`deskList`), never a session that
  // only once was a desk or whose agent left the roster.
  const roster = useAgents()
  const deskMap = useMemo(() => deskSessions(deskList(roster.data ?? [])), [roster.data])
  const [picked, setPicked] = useState<Set<string> | null>(null)
  const [delivery, setDelivery] = useState<Delivery>('steer')
  const [preview, setPreview] = useState<Preview | null>(null)
  // State, not a ref: `SessionList`'s virtualizer reads its scroll element in a layout effect (as in `AppSidebar`).
  const [scrollEl, setScrollEl] = useState<HTMLDivElement | null>(null)

  // Untouched selection follows the rows until the first click: the preset applies to whatever loads (a desks preset
  // also waits for the roster, which is where the desks come from).
  const initial = (rows: AgentInfo[]) => {
    if (req.preset === 'roots') return allRoots(rows)
    if (req.preset === 'desks') {
      const desks = deskRoots(rows, deskMap)
      if (desks.length > 0 || !roster.data) return desks
    }
    return runningRoots(rows)
  }
  const ids = picked ?? new Set(rows ? initial(rows) : [])
  const key = [...ids].sort().join()

  useEffect(() => {
    if (ids.size === 0) {
      setPreview(null)
      return
    }
    let live = true
    const timer = window.setTimeout(() => {
      api.broadcast({ targets: { kind: 'sessions', ids: [...ids] }, text: '', files: [], delivery, dry_run: true }).then(
        (receipt) => live && setPreview({ receipt }),
        (e: unknown) => live && setPreview({ error: e instanceof Error ? e.message : String(e) }),
      )
    }, 300)
    return () => {
      live = false
      window.clearTimeout(timer)
    }
    // `key` stands for `ids`; a new Set per render must not retrigger.
  }, [api, key, delivery])

  const tick: Ticks = {
    ids,
    toggle: (id) => {
      const next = new Set(ids)
      if (!next.delete(id)) next.add(id)
      setPicked(next)
    },
  }

  // The composer's send: one broadcast to the ticked sessions. A throw keeps the draft (flagged unsent) and toasts.
  const deliver = async ({ text, files }: { text: string; files: string[] }) => {
    if (ids.size === 0) throw new Error('Pick at least one session')
    const receipt = await api.broadcast({ targets: { kind: 'sessions', ids: [...ids] }, text, files, delivery, dry_run: false })
    const failed = receipt.rows.filter((r) => r.outcome.outcome === 'failed').length
    const lines = outcomeLines(receipt, sessions)
    ;(failed > 0 ? toast.warning : toast.success)(
      failed > 0 ? `Sent to ${receipt.rows.length - failed} of ${receipt.rows.length} sessions` : `Sent to ${receipt.rows.length} sessions`,
      { description: lines.join('\n') },
    )
    return ''
  }

  const n = ids.size
  const deskIds = rows ? deskRoots(rows, deskMap) : []
  return (
    <>
      <section className="space-y-2 text-sm">
        <Composer session={null} draftScope={BROADCAST_DRAFT} deliver={deliver} onSent={onDone} />
        <ToggleGroup
          type="single"
          variant="outline"
          value={delivery}
          onValueChange={(v) => v && setDelivery(v as Delivery)}
          aria-label="Delivery"
        >
          <ToggleGroupItem value="steer">Steer (inside the running turn)</ToggleGroupItem>
          <ToggleGroupItem value="next_turn">Next turn</ToggleGroupItem>
        </ToggleGroup>
        <p className={cn('text-xs', preview && 'error' in preview ? 'text-danger' : 'text-fg-tertiary')}>
          {preview === null
            ? n === 0
              ? 'Pick at least one session'
              : 'Previewing…'
            : 'error' in preview
              ? preview.error
              : summarize(preview.receipt)}
        </p>
      </section>
      <section className="flex min-h-0 flex-1 flex-col text-sm">
        <div className="mb-1 flex flex-wrap items-center gap-2">
          <h3 className="font-medium">Targets</h3>
          <span className="text-xs tabular-nums text-fg-tertiary">{n} ticked</span>
          <div className="ml-auto flex gap-1">
            {deskIds.length > 0 && <Chip onClick={() => setPicked(new Set(deskIds))}>All desks</Chip>}
            <Chip onClick={() => rows && setPicked(new Set(runningRoots(rows)))}>Running roots</Chip>
            <Chip onClick={() => rows && setPicked(new Set(allRoots(rows)))}>All roots</Chip>
            <Chip onClick={() => setPicked(new Set())}>None</Chip>
          </div>
        </div>
        {loadError && <p className="text-danger">{loadError instanceof Error ? loadError.message : String(loadError)}</p>}
        <div ref={setScrollEl} className="-mx-2 flex min-h-0 flex-1 flex-col overflow-y-auto">
          <Capability required={{ methods: ['desk_open', 'list_sessions'] }} hide>
            <SidebarMenu className="px-1.5">
              <Desks active={active} tick={tick} />
            </SidebarMenu>
          </Capability>
          <SessionList scrollEl={scrollEl} barEl={null} active={active} tick={tick} />
        </div>
      </section>
    </>
  )
}

function Chip({ onClick, children }: { onClick(): void; children: string }) {
  return (
    <Button variant="outline" size="sm" className="h-7 rounded-full px-2.5 text-xs" onClick={onClick}>
      {children}
    </Button>
  )
}
