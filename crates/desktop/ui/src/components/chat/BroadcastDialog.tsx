import { useQuery } from '@tanstack/react-query'
import { type ReactNode, useEffect, useState } from 'react'
import { toast } from 'sonner'
import { GLYPH, ago } from '@/components/sidebar/AgentTree'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Sheet, SheetContent, SheetDescription, SheetFooter, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { Textarea } from '@/components/ui/textarea'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { useIsMobile } from '@/hooks/use-mobile'
import { useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { BroadcastReceipt } from '@/rpc/bindings/BroadcastReceipt'
import type { Delivery } from '@/rpc/bindings/Delivery'
import type { AgentInfo } from '@/rpc/extra'
import { allRoots, busy, counts, groupRows, outcomeLines, runningRoots, summarize, title } from './broadcast'

type Request = {
  /** Prefills the message. */
  text?: string
  /** Initial tick set; `running` when omitted (62 §0.2). */
  preset?: 'running' | 'roots'
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
        <SheetContent side="bottom" className="max-h-[90vh] gap-2 px-4">
          <SheetHeader className="px-0">
            <SheetTitle>Broadcast</SheetTitle>
            <SheetDescription>{DESCRIPTION}</SheetDescription>
          </SheetHeader>
          {req && <Form req={req} onDone={close} Footer={SheetFooter} />}
        </SheetContent>
      </Sheet>
    )
  }
  return (
    <Dialog open={req !== null} onOpenChange={(o) => !o && close()}>
      <DialogContent className="flex max-h-[85vh] flex-col sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>Broadcast</DialogTitle>
          <DialogDescription>{DESCRIPTION}</DialogDescription>
        </DialogHeader>
        {req && <Form req={req} onDone={close} Footer={DialogFooter} />}
      </DialogContent>
    </Dialog>
  )
}

type Preview = { receipt: BroadcastReceipt } | { error: string }

/** Targets, message, delivery in one scroll; the footer previews with `dry_run: true` and sends with one click. */
function Form({
  req,
  onDone,
  Footer,
}: {
  req: Request
  onDone(): void
  Footer: (p: { children: ReactNode; className?: string }) => ReactNode
}) {
  const api = useApi()
  const { data: rows, error: loadError } = useQuery({ queryKey: ['activity'], queryFn: () => api.list_activity() })
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

  // Untouched selection follows the rows until the first click: the preset applies to whatever loads.
  const ids = picked ?? new Set(rows ? (req.preset === 'roots' ? allRoots(rows) : runningRoots(rows)) : [])
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
      const lines = outcomeLines(receipt)
      ;(failed > 0 ? toast.warning : toast.success)(
        failed > 0 ? `Sent to ${receipt.rows.length - failed} of ${receipt.rows.length} sessions` : `Sent to ${receipt.rows.length} sessions`,
        { description: <pre className="whitespace-pre-wrap font-sans">{lines.join('\n')}</pre> },
      )
      onDone()
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    } finally {
      setSending(false)
    }
  }

  const n = ids.size
  const groups = rows ? groupRows(rows) : []
  return (
    <>
      <div className="-mx-2 min-h-0 flex-1 space-y-4 overflow-auto px-2 text-sm">
        <section>
          <div className="mb-1 flex items-center gap-2">
            <h3 className="font-medium">Targets</h3>
            <div className="ml-auto flex gap-1">
              <Chip onClick={() => rows && setPicked(new Set(runningRoots(rows)))}>Running roots</Chip>
              <Chip onClick={() => rows && setPicked(new Set(allRoots(rows)))}>All roots</Chip>
              <Chip onClick={() => setPicked(new Set())}>None</Chip>
            </div>
          </div>
          {loadError && <p className="text-danger">{loadError instanceof Error ? loadError.message : String(loadError)}</p>}
          {rows && rows.length === 0 && <p className="text-fg-tertiary">No live sessions</p>}
          <ul aria-label="Sessions">
            {groups.map((g) => (
              <li key={g.root.id}>
                <Row row={g.root} checked={ids.has(g.root.id)} now={now} onToggle={toggle} />
                {g.children.length > 0 && (
                  <ul>
                    {g.children.map((c) => (
                      <li key={c.id}>
                        <Row row={c} checked={ids.has(c.id)} now={now} onToggle={toggle} />
                      </li>
                    ))}
                  </ul>
                )}
              </li>
            ))}
          </ul>
        </section>
        <section>
          <h3 className="mb-1 font-medium">Message</h3>
          <Textarea
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder="What every selected session should hear"
            rows={4}
            autoFocus={!req.text}
          />
        </section>
        <section>
          <h3 className="mb-1 font-medium">Delivery</h3>
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
        </section>
      </div>
      <Footer className="items-center gap-2 sm:justify-between">
        <span className={cn('min-w-0 flex-1 text-xs', preview && 'error' in preview ? 'text-danger' : 'text-fg-tertiary')}>
          {preview === null
            ? n === 0
              ? 'Pick at least one session'
              : 'Previewing…'
            : 'error' in preview
              ? preview.error
              : summarize(preview.receipt)}
        </span>
        <Button disabled={n === 0 || !text.trim() || sending} onClick={() => void send()}>
          {sending ? 'Sending…' : `Send to ${n} session${n === 1 ? '' : 's'}`}
        </Button>
      </Footer>
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

/** Checkbox row, 44 px tall for fingers (48 §2.3); children indent by depth like `AgentTree`. */
function Row({ row, checked, now, onToggle }: { row: AgentInfo; checked: boolean; now: number; onToggle(id: string): void }) {
  const a = row.activity
  const state = a?.state.state ?? row.state.state
  const extra = a && counts(a)
  const age = ago(a?.last_active ?? null, now)
  return (
    <label
      className="flex min-h-11 cursor-pointer items-center gap-3 rounded-lg py-2 pr-2 hover:bg-layer-2 has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-ring"
      style={{ paddingLeft: 8 + row.depth * 16 }}
    >
      <input type="checkbox" className="size-4 shrink-0 accent-foreground" checked={checked} onChange={() => onToggle(row.id)} />
      <span className="min-w-0 flex-1 truncate">{title(row)}</span>
      <span
        className={cn('shrink-0 rounded-md bg-layer-2 px-1.5 text-xs text-fg-secondary', busy(a) && 'text-foreground')}
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
