// 227 piece 3: the "Set on all machines…" sheet: pick machines, apply, then one result row per machine (mock).

import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { MachineRow } from '@/components/settings/MachineRow'
import { type SettingsMachines, usePickedMachine } from '@/components/settings/machine-target'
import type { SettingsNeed } from '@/components/settings/scope'
import { errorText } from '@/components/settings/shared'
import { type Choice, type CopyField, type ResultRow, choices, resultRow, runApis, runPlan, settlePending } from '@/components/settings/copy/set-on-all'
import { useIsMobile } from '@/hooks/use-mobile'
import type { Machine } from '@/lib/mesh-machines'
import { cn } from '@/lib/utils'

const MARK_TONE: Readonly<Record<ResultRow['mark'], string>> = { '✓': 'text-success', '◷': 'text-warning', '–': 'text-fg-tertiary', '✗': 'text-destructive' }

/** What the sheet copies: the field, the value (the edited machine's, or the one "Use this everywhere" picked) and where it lives. */
export type SetOnAllRequest<T = unknown> = { field: CopyField<T>; value: T; tab: string; needs?: readonly SettingsNeed[] }

export function SetOnAllSheet({ request, settings, onClose }: { request: SetOnAllRequest | null; settings: SettingsMachines; onClose: () => void }) {
  const mobile = useIsMobile()
  return (
    <Sheet open={!!request} onOpenChange={(o) => !o && onClose()}>
      <SheetContent side={mobile ? 'bottom' : 'right'} className={cn('gap-0 overflow-y-auto', mobile && 'max-h-[85dvh] rounded-t-2xl')}>
        {request && <SetOnAll key={`${request.tab}/${request.field.id}/${JSON.stringify(request.value)}`} request={request} settings={settings} onClose={onClose} />}
      </SheetContent>
    </Sheet>
  )
}

/** The sheet's body; `SetOnAllSheet` keys it per request so a new request starts unticked-fresh. `onClose` closes the sheet. */
export function SetOnAll({ request, settings, onClose }: { request: SetOnAllRequest; settings: SettingsMachines; onClose: () => void }) {
  const { field, value } = request
  const plan = field.plan(value)
  const display = field.format(value)
  const [picks, setPicks] = useState<Choice[]>(() =>
    choices(settings.machines, { needs: request.needs, hubOnly: field.hubOnly, skip: plan.kind === 'providers' ? plan.from : undefined }),
  )
  const [rows, setRows] = useState<ResultRow[] | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const entry = { id: `${request.tab}/${field.id}`, label: field.label, display, plan }

  const run = async (targets: Machine[], keep: ResultRow[], fresh = false) => {
    setBusy(true)
    setError(null)
    try {
      const results = await runPlan(runApis(settings), plan, targets)
      settlePending(entry, results, fresh)
      const byEngine = new Map(results.map((r) => [r.engine, resultRow(r)]))
      setRows(keep.map((row) => byEngine.get(row.engine) ?? row))
    } catch (e) {
      setError(errorText(e))
    } finally {
      setBusy(false)
    }
  }

  const apply = () => {
    const ticked = picks.filter((c) => c.ticked).map((c) => c.machine)
    const skipped = picks.flatMap((c): ResultRow[] => (c.disabled ? [resultRow({ engine: c.machine.id, name: c.machine.name, status: 'skipped', message: c.disabled })] : []))
    const placeholders = ticked.map((m): ResultRow => ({ engine: m.id, name: m.name, status: 'ok', mark: '◷', text: 'applying…', restart: false, pending: false, press: false }))
    const order = new Map(settings.machines.map((m, i) => [m.id, i]))
    const all = [...placeholders, ...skipped].sort((a, b) => order.get(a.engine)! - order.get(b.engine)!)
    setRows(all)
    void run(ticked, all, true)
  }

  const pending = rows?.filter((r) => r.pending) ?? []
  const retry = () => void run(settings.machines.filter((m) => pending.some((r) => r.engine === m.id)), rows ?? [])
  /**
   * `needs_press`: the change must be pressed on that machine itself (a forwarded call is never a press). A machine
   * this app reaches with a client of its own: pick it and close the sheet, leaving its tab open to press there.
   */
  const openOn = (m: Machine) => {
    usePickedMachine.getState().pick(m.connected ? null : m.id)
    onClose()
  }
  const reachable = (engine: string) => settings.machines.find((m) => m.id === engine && !m.forwarded)

  return (
    <div data-set-on-all="" className="flex flex-col gap-4 p-4">
      <SheetHeader className="p-0">
        <SheetTitle>{rows ? `${field.label} → ${display}` : 'Set on all machines'}</SheetTitle>
        {!rows && (
          <SheetDescription>
            {field.label} → <span className="font-medium text-foreground">{display}</span>
          </SheetDescription>
        )}
      </SheetHeader>
      {rows ? (
        <div data-set-on-all-results="" className="rounded-[12px] border border-border-subtle bg-layer-2 px-3 py-2">
          <div className="flex items-center justify-end pb-1">
            <Button size="sm" variant="outline" disabled={busy || pending.length === 0} onClick={retry}>
              Retry pending
            </Button>
          </div>
          <ul className="space-y-0.5 text-sm">
            {rows.map((row) => (
              <li key={row.engine} data-result={row.status} className="flex items-center gap-2 py-1">
                <span aria-hidden className={cn('w-4 shrink-0 text-center', MARK_TONE[row.mark])}>
                  {row.mark}
                </span>
                <span className="min-w-0 truncate">{row.name}</span>
                {row.restart && <span data-restart-chip="" className="shrink-0 rounded-full border border-warning/40 px-1.5 text-[10px] leading-4 text-warning">restart needed</span>}
                <small className="ml-auto text-right text-[12px] text-fg-tertiary">{row.text}</small>
                {row.press &&
                  (reachable(row.engine) ? (
                    <Button size="sm" variant="outline" data-open-on={row.engine} className="h-6 shrink-0 px-2 text-[11px]" onClick={() => openOn(reachable(row.engine)!)}>
                      Open on {row.name}
                    </Button>
                  ) : (
                    <small data-open-theuth-on={row.engine} className="shrink-0 text-[11px] text-fg-tertiary">
                      open theuth on {row.name} to finish
                    </small>
                  ))}
              </li>
            ))}
          </ul>
          {pending.length > 0 && (
            <p className="mt-2 text-[12px] text-fg-tertiary">Pending copies are kept by this app, not by the machines: they are retried while it is open, when each machine is back.</p>
          )}
          {error && <p className="mt-2 text-[13px] text-destructive">{error}</p>}
        </div>
      ) : (
        <>
          <ul className="space-y-1">
            {picks.map((c) => (
              <li key={c.machine.id}>
                <label data-choice={c.machine.id} className={cn('flex items-center gap-3 rounded-[10px] px-2 py-2 text-sm', c.disabled ? 'opacity-60' : 'hover:bg-layer-3/60')}>
                  <input
                    type="checkbox"
                    className="size-4 accent-[var(--primary)]"
                    checked={c.ticked}
                    disabled={!!c.disabled}
                    onChange={(e) => {
                      const ticked = e.currentTarget.checked
                      setPicks((ps) => ps.map((p) => (p.machine.id === c.machine.id ? { ...p, ticked } : p)))
                    }}
                  />
                  <MachineRow machine={c.machine} tag={c.disabled ?? c.note ?? undefined} />
                </label>
              </li>
            ))}
          </ul>
          <div className="flex justify-end">
            <Button size="sm" disabled={busy || !picks.some((c) => c.ticked)} onClick={apply}>
              Apply to {picks.filter((c) => c.ticked).length} machine{picks.filter((c) => c.ticked).length === 1 ? '' : 's'}
            </Button>
          </div>
        </>
      )}
    </div>
  )
}
