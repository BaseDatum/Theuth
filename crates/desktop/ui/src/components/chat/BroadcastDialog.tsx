import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery } from '@tanstack/react-query'
import { useEffect, useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { Branch, GLYPH, ago } from '@/components/sidebar/AgentTree'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { Textarea } from '@/components/ui/textarea'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { useIsMobile } from '@/hooks/use-mobile'
import { queryKeys, useApi } from '@/lib/rpc'
import { childrenOf } from '@/lib/session-tree'
import { cn } from '@/lib/utils'
import type { BroadcastReceipt } from "@/rpc/bindings/method.agent.output"
import type { Delivery } from "@/rpc/bindings/method.agent.output"
import type { SessionSummary } from "@/rpc/bindings/method.agent.output"
import type { AgentInfo } from '@/store/ui-events'
import {
  active,
  allRoots,
  busy,
  counts,
  deskOf,
  deskRoots,
  type Group,
  groupRows,
  isRoot,
  outcomeLines,
  runningRoots,
  sectionState,
  splitGroups,
  summarize,
  title,
} from './broadcast'

type Request = {
  /** Prefills the message. */
  text?: string
  /** Initial tick set; `running` when omitted (62 §0.2). `desks` falls back to `running` when no desk is open. */
  preset?: 'running' | 'roots' | 'desks'
}

/** Open the broadcast dialog (62 §5.2): the sidebar action, `/broadcast` and the composer's "Send to many…" all end here. */
export function openBroadcastDialog(req: Request = {}): void {
  window.dispatchEvent(new CustomEvent<Request>('theuth:broadcast', { detail: req }))
}

const DESCRIPTION = 'One message to many sessions; each recipient sees it marked as a broadcast.'

/** Mounted once beside the sidebar. `Dialog` on desktop, bottom sheet on phones (the sidebar's own split). */
export function BroadcastDialog() {
  const [req, setReq] = useState<Request | null>(null)
  const isMobile = useIsMobile()
  useEffect(() => {
    const on = (e: Event) => setReq((e as CustomEvent<Request>).detail)
    window.addEventListener('theuth:broadcast', on)
    return () => window.removeEventListener('theuth:broadcast', on)
  }, [])
  const close = () => setReq(null)
  if (isMobile) {
    return (
      <Sheet open={req !== null} onOpenChange={(o) => !o && close()}>
        <SheetContent side="bottom" className="max-h-[90vh] gap-2 px-4 pb-[calc(1rem+var(--safe-bottom))]">
          <SheetHeader className="px-0">
            <SheetTitle>Broadcast</SheetTitle>
            <SheetDescription>{DESCRIPTION}</SheetDescription>
          </SheetHeader>
          {req && <Form req={req} onDone={close} />}
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
        {req && <Form req={req} onDone={close} />}
      </DialogContent>
    </Dialog>
  )
}

type Preview = { receipt: BroadcastReceipt } | { error: string }

/** Message, delivery, Send and the `dry_run: true` preview on top; only the targets list below them scrolls. */
export function Form({ req, onDone }: { req: Request; onDone(): void }) {
  const canQuery1 = useQuerySupport(["list_activity"])
  const canQuery1_2 = useQuerySupport(["list_sessions"])
  const api = useApi()
  const { data: rows, error: loadError } = useQuery({ enabled: canQuery1, queryKey: ['activity'], queryFn: () => api.list_activity() })
  const { data: sessions } = useQuery({ enabled: canQuery1_2, queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() })
  const [picked, setPicked] = useState<Set<string> | null>(null)
  const [text, setText] = useState(req.text ?? '')
  const [delivery, setDelivery] = useState<Delivery>('steer')
  const [preview, setPreview] = useState<Preview | null>(null)
  const [sending, setSending] = useState(false)
  const [now, setNow] = useState(Date.now())
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1_000)
    return () => window.clearInterval(timer)
  }, [])

  // Untouched selection follows the rows until the first click: the preset applies to whatever loads (a desks preset
  // also waits for the session list, which is where `desk` lives).
  const initial = (rows: AgentInfo[]) => {
    if (req.preset === 'roots') return allRoots(rows)
    if (req.preset === 'desks') {
      const desks = deskRoots(rows, sessions)
      if (desks.length > 0 || !sessions) return desks
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
      api.broadcast({ targets: { kind: 'sessions', ids: [...ids] }, text, files: [], delivery, dry_run: true }).then(
        (receipt) => live && setPreview({ receipt }),
        (e: unknown) => live && setPreview({ error: e instanceof Error ? e.message : String(e) }),
      )
    }, 300)
    return () => {
      live = false
      window.clearTimeout(timer)
    }
    // `key` stands for `ids`; a new Set per render must not retrigger.
  }, [api, key, text, delivery])

  const toggle = (id: string) => {
    const next = new Set(ids)
    if (!next.delete(id)) next.add(id)
    setPicked(next)
  }

  const send = async () => {
    setSending(true)
    try {
      const receipt = await api.broadcast({
        targets: { kind: 'sessions', ids: [...ids] },
        text,
        files: [],
        delivery,
        dry_run: false,
      })
      const failed = receipt.rows.filter((r) => r.outcome.outcome === 'failed').length
      const lines = outcomeLines(receipt, sessions)
      ;(failed > 0 ? toast.warning : toast.success)(
        failed > 0 ? `Sent to ${receipt.rows.length - failed} of ${receipt.rows.length} sessions` : `Sent to ${receipt.rows.length} sessions`,
        { description: lines.join('\n') },
      )
      onDone()
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    } finally {
      setSending(false)
    }
  }

  const n = ids.size
  const { desks, rest } = splitGroups(rows ? groupRows(rows) : [], sessions)
  const deskIds = desks.map((g) => g.root.id)
  const section = { ids, now, sessions, onToggle: toggle, onPick: setPicked }
  return (
    <>
      <section className="space-y-2 text-sm">
        <h3 className="font-medium">Message</h3>
        <Textarea
          value={text}
          onChange={(e) => setText(e.target.value)}
          placeholder="What every selected session should hear"
          rows={4}
          autoFocus={!req.text}
        />
        <div className="flex flex-wrap items-center gap-2">
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
          <Button className="ml-auto" disabled={n === 0 || !text.trim() || sending} onClick={() => void send()}>
            {sending ? 'Sending…' : `Send to ${n} session${n === 1 ? '' : 's'}`}
          </Button>
        </div>
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
          <div className="ml-auto flex gap-1">
            {deskIds.length > 0 && <Chip onClick={() => setPicked(new Set(deskIds))}>All desks</Chip>}
            <Chip onClick={() => rows && setPicked(new Set(runningRoots(rows)))}>Running roots</Chip>
            <Chip onClick={() => rows && setPicked(new Set(allRoots(rows)))}>All roots</Chip>
            <Chip onClick={() => setPicked(new Set())}>None</Chip>
          </div>
        </div>
        <div className="-mx-2 min-h-0 flex-1 overflow-auto px-2">
          {loadError && <p className="text-danger">{loadError instanceof Error ? loadError.message : String(loadError)}</p>}
          {rows && rows.length === 0 && <p className="text-fg-tertiary">No live sessions</p>}
          {desks.length > 0 && <Section label="Desks" groups={desks} {...section} />}
          {rest.length > 0 && <Section label="Sessions" groups={rest} {...section} />}
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

/**
 * One section of the targets list (D2): a header row with a tri-state checkbox that adds the section's roots to the
 * selection, or removes them when they are all ticked — never touching the other section — then its groups.
 */
function Section({
  label,
  groups,
  ids,
  now,
  sessions,
  onToggle,
  onPick,
}: {
  label: string
  groups: Group[]
  ids: Set<string>
  now: number
  sessions: SessionSummary[] | undefined
  onToggle(id: string): void
  onPick(ids: Set<string>): void
}) {
  const rootIds = groups.map((g) => g.root.id)
  const state = sectionState(ids, rootIds)
  const picked = rootIds.filter((id) => ids.has(id)).length
  const flip = () => {
    const next = new Set(ids)
    for (const id of rootIds) {
      if (state === 'all') next.delete(id)
      else next.add(id)
    }
    onPick(next)
  }
  return (
    <div>
      <label className="flex min-h-11 cursor-pointer items-center gap-3 rounded-lg py-2 pl-2 pr-2 hover:bg-layer-2 has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-ring">
        <input
          type="checkbox"
          className="size-4 shrink-0 accent-foreground"
          checked={state === 'all'}
          ref={(el) => {
            if (el) el.indeterminate = state === 'some'
          }}
          onChange={flip}
          aria-label={`All ${label.toLowerCase()}`}
        />
        <span className="min-w-0 flex-1 font-medium">{label}</span>
        <span className="shrink-0 text-xs tabular-nums text-fg-tertiary">
          {picked} of {rootIds.length}
        </span>
      </label>
      <ul aria-label={label}>
        {groups.map((g) => (
          <li key={g.root.id}>
            <Row
              row={g.root}
              checked={ids.has(g.root.id)}
              now={now}
              sessions={sessions}
              onToggle={onToggle}
              subagents={g.children.filter((c) => active(c.activity)).length}
            />
            <Branch
              parent={g.root.id}
              byParent={childrenOf(g.children)}
              sessions={sessions ?? []}
              unit={16}
              moreClass="flex min-h-11 w-full items-center rounded-lg py-2 pr-2 text-left text-fg-tertiary hover:bg-layer-2"
              render={(r, sub) => (
                <>
                  <Row row={r} checked={ids.has(r.id)} now={now} sessions={sessions} onToggle={onToggle} />
                  {sub}
                </>
              )}
            />
          </li>
        ))}
      </ul>
    </div>
  )
}

/** Checkbox row, 44 px tall for fingers (48 §2.3); children indent by depth like `AgentTree`. */
function Row({
  row,
  checked,
  now,
  sessions,
  onToggle,
  subagents = 0,
}: {
  row: AgentInfo
  checked: boolean
  now: number
  sessions: SessionSummary[] | undefined
  onToggle(id: string): void
  /** A root's `active` descendants: an idle root with any reads as running, as "Running roots" ticks it. */
  subagents?: number
}) {
  const a = row.activity
  const state = a?.state.state ?? row.state.state
  const sub = subagents > 0 ? `${subagents} subagent${subagents === 1 ? '' : 's'} active` : ''
  const extra = [a && counts(a), sub].filter(Boolean).join(' · ')
  const age = ago(a?.last_active ?? null, now)
  const desk = isRoot(row) ? deskOf(row.id, sessions) : undefined
  return (
    <label
      className="flex min-h-11 cursor-pointer items-center gap-3 rounded-lg py-2 pr-2 hover:bg-layer-2 has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-ring"
      style={{ paddingLeft: 8 + row.depth * 16 }}
    >
      <input type="checkbox" className="size-4 shrink-0 accent-foreground" checked={checked} onChange={() => onToggle(row.id)} />
      <span className="min-w-0 flex-1 truncate">{desk ?? title(row.id, sessions, row.name)}</span>
      {desk && <span className="shrink-0 rounded-md bg-layer-2 px-1.5 text-[10px] uppercase tracking-wide text-fg-secondary">desk</span>}
      <span
        className={cn('shrink-0 rounded-md bg-layer-2 px-1.5 text-xs text-fg-secondary', (busy(a) || subagents > 0) && 'text-foreground')}
        title={state}
      >
        <span className={cn('mr-1', state === 'running' && 'animate-pulse')}>{GLYPH[state]}</span>
        {state}
      </span>
      {(extra || age) && (
        <span className="shrink-0 text-xs tabular-nums text-fg-tertiary">{[extra, age].filter(Boolean).join(' · ')}</span>
      )}
    </label>
  )
}
