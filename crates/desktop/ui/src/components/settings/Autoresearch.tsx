import { Link } from '@tanstack/react-router'
import { FlaskConical } from 'lucide-react'
import { useEffect } from 'react'
import { useModels } from '@/components/agents/queries'
import { KEY, useAutoresearchApi, useAutoresearchSettings, useMechanism } from '@/components/autoresearch/queries'
import { ModelCombobox } from '@/components/chat/ModelCombobox'
import { Mechanism } from '@/components/settings/Mechanism'
import { Count, Empty, Row, Section, bounded, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { useRestartPending } from '@/lib/restart-pending'
import { RpcError } from '@/rpc/client'
import type { AutoresearchConfig } from "@/rpc/bindings/method.autoresearch.output"

type Role = 'executor' | 'review' | 'advisor'
const ROLES: Role[] = ['executor', 'review', 'advisor']
const ROLE_LABEL: Record<Role, string> = { executor: 'Executor', review: 'Reviewer', advisor: 'Advisor' }
const ROLE_HELP: Record<Role, string> = {
  executor: 'Runs the study session: reads, edits, calls the experiment tools. Default is the session default model (R1-D5).',
  review: 'Judges every kept diff with the fixed rubric. Default is a configured model of another vendor than the executor, else the executor.',
  advisor: 'Consulted once at a plateau for five materially different ideas. Default is the Advisor settings model, else the executor at high effort.',
}
const ROLE_NONE: Record<Role, string> = { executor: 'Session default', review: 'Other vendor (auto)', advisor: 'Advisor default' }

/** `models.executor` / `models.advisor` and `review.model` (§12.2) read as one picker row each. */
const roleModel = (c: AutoresearchConfig, role: Role): string | null => (role === 'review' ? c.review.model : c.models[role])
const withRoleModel = (c: AutoresearchConfig, role: Role, model: string | null): AutoresearchConfig => (role === 'review' ? { ...c, review: { ...c.review, model } } : { ...c, models: { ...c.models, [role]: model } })

/** `600s` / `5m` / `1h` ↔ whole seconds (the spec's durations are humantime strings). */
export function secondsOf(d: string): number | null {
  const m = /^\s*(\d+(?:\.\d+)?)\s*(ms|s|m|min|h)?\s*$/.exec(d)
  if (!m) return null
  const v = Number(m[1])
  const unit = m[2] ?? 's'
  return Math.round(unit === 'ms' ? v / 1000 : unit === 's' ? v : unit === 'h' ? v * 3600 : v * 60)
}

/** The engine's `NOT_RUNNING` JSON-RPC code: a plugin method called while the plugin is off. */
const NOT_RUNNING = -32001

/**
 * 188 F3: whether the saved `enabled` waits for a restart. The plugin runs now when `autoresearch.mechanism {op: list}`
 * (the Mechanism section's own query) answers, and is off when it answers `NOT_RUNNING`; `autoresearch.list` cannot
 * tell, it answers `[]` either way. `null` while either side is unknown (loading, or another error).
 */
export function awaitsRestart(enabled: boolean | undefined, mechanism: { isSuccess: boolean; error: unknown }): boolean | null {
  const running = mechanism.isSuccess ? true : mechanism.error instanceof RpcError && mechanism.error.code === NOT_RUNNING ? false : null
  return enabled === undefined || running === null ? null : enabled !== running
}

/** Settings › Agent › Autoresearch (140 §13.1): the plugin switch (188 F3), defaults, model pickers, reviewer action, bench lock + load threshold, keep-finished days; then the W7 Mechanism section. Reads/writes `autoresearch.settings.get|set`. */
export function Autoresearch() {
  const api = useAutoresearchApi()
  const settings = useAutoresearchSettings()
  const models = useModels()
  const save = useAction((next: AutoresearchConfig) => api['autoresearch.settings.set'](next), { keys: [[...KEY, 'settings']] })
  const s = settings.data
  const busy = !s || save.isPending
  /** Save the settings with one change applied; every control routes through here. */
  const change = (f: (cur: AutoresearchConfig) => AutoresearchConfig) => {
    if (s) save.mutate(f(s))
  }
  const count = (f: (cur: AutoresearchConfig, n: number) => AutoresearchConfig) => ({ mutate: (n: number) => change((cur) => f(cur, n)), isPending: save.isPending })
  const restart = awaitsRestart(s?.enabled, useMechanism())
  useEffect(() => {
    if (restart === null) return
    const pending = useRestartPending.getState()
    if (restart) pending.mark('autoresearch.enabled', 'Autoresearch')
    else pending.clear('autoresearch.enabled')
  }, [restart])

  return (
    <>
      <Section
        title="Autoresearch"
        actions={
          <Button asChild variant="outline" size="sm">
            <Link to="/autoresearch" search={{}}>
              <FlaskConical /> Studies
            </Link>
          </Button>
        }
      >
        {settings.isError ? <Empty>{settings.error.message}</Empty> : null}
        <Row
          label="Enabled"
          description={
            <>
              {s?.enabled === false ? 'Off: no study runs and the study tools are gone. Existing studies stay paused.' : 'Studies, their tools and their routine. Start one from any chat with /autoresearch <goal>.'}
              {restart && <span className="block">Applies after theuth restarts.</span>}
            </>
          }
        >
          <Switch aria-label="Autoresearch enabled" checked={s?.enabled ?? false} disabled={busy} onCheckedChange={(enabled) => change((c) => ({ ...c, enabled }))} />
        </Row>
        <Row label="Budget per study" description="Pre-filled in every draft card; the card lets you change them before Start. Money · hours · experiments.">
          <Money value={s?.defaults.budget.usd} aria-label="Budget USD" disabled={busy} onCommit={(usd) => change((c) => ({ ...c, defaults: { ...c.defaults, budget: { ...c.defaults.budget, usd } } }))} />
          <Count value={s?.defaults.budget.hours} min={1} max={168} action={count((c, hours) => ({ ...c, defaults: { ...c.defaults, budget: { ...c.defaults.budget, hours } } }))} />
          <Count value={s?.defaults.budget.experiments} min={1} max={5000} action={count((c, experiments) => ({ ...c, defaults: { ...c.defaults, budget: { ...c.defaults.budget, experiments } } }))} />
        </Row>
        <Row label="Budget per experiment" description="The per-fire money cap (136 budget:). A fire past it ends at once and the experiment closes unlogged.">
          <Money value={s?.defaults.budget.per_experiment_usd} aria-label="Per-experiment USD" disabled={busy} onCommit={(per_experiment_usd) => change((c) => ({ ...c, defaults: { ...c.defaults, budget: { ...c.defaults.budget, per_experiment_usd } } }))} />
        </Row>
        <Row label="Baseline repeats" description="Samples of the base at Start and of each changeset at the end; σ comes from these and every confirm run.">
          <Count value={s?.defaults.repeats} min={2} max={20} action={count((c, repeats) => ({ ...c, defaults: { ...c.defaults, repeats } }))} />
        </Row>
        <Row label="Timeouts" description="One benchmark sample · the correctness check, in seconds. A sample past its timeout is a crash.">
          <Seconds value={s?.defaults.bench_timeout} aria-label="Benchmark timeout in seconds" disabled={busy} onCommit={(d) => change((c) => ({ ...c, defaults: { ...c.defaults, bench_timeout: d } }))} />
          <Seconds value={s?.defaults.checks_timeout} aria-label="Checks timeout in seconds" disabled={busy} onCommit={(d) => change((c) => ({ ...c, defaults: { ...c.defaults, checks_timeout: d } }))} />
        </Row>
      </Section>

      <Section title="Models">
        {ROLES.map((role) => {
          const off = role === 'review' && s?.review.model === 'off'
          const current = s ? models.data?.find((m) => m.id === roleModel(s, role)) : undefined
          return (
            <Row stack key={role} label={ROLE_LABEL[role]} description={ROLE_HELP[role]}>
              {role === 'review' && (
                <ToggleGroup type="single" variant="outline" size="sm" value={off ? 'off' : 'on'} aria-label="Reviewer" disabled={busy} onValueChange={(v) => v && change((c) => withRoleModel(c, 'review', v === 'off' ? 'off' : null))}>
                  <ToggleGroupItem value="on">On</ToggleGroupItem>
                  <ToggleGroupItem value="off" title="review.model: off — keeps are not checked by a second model; the draft card warns">
                    Off
                  </ToggleGroupItem>
                </ToggleGroup>
              )}
              {!off && (
                <ModelCombobox
                  aria-label={`${ROLE_LABEL[role]} model`}
                  className="w-full max-w-72 max-md:max-w-none"
                  models={models.data ?? []}
                  value={s ? roleModel(s, role) : undefined}
                  // 194 row 19: each role's effort sits beside its model (`models.<role>_effort`, `review.effort`).
                  effort={
                    s && current && current.efforts.length > 0
                      ? {
                          levels: current.efforts,
                          current: role === 'review' ? s.review.effort : s.models[`${role}_effort` as const],
                          onSelect: (effort) => change((c) => (role === 'review' ? { ...c, review: { ...c.review, effort } } : { ...c, models: { ...c.models, [`${role}_effort`]: effort } })),
                        }
                      : undefined
                  }
                  none={{ label: ROLE_NONE[role] }}
                  disabled={busy}
                  onChange={(v) => change((c) => withRoleModel(c, role, v))}
                />
              )}
            </Row>
          )
        })}
      </Section>

      <Section title="Review and the machine">
        <Row label="A keep the reviewer calls gaming" description="Undo reverts it, keeps the patch and raises an inbox row with Restore (R1-D2). Flag only keeps the change and marks it.">
          <ToggleGroup type="single" variant="outline" size="sm" value={s?.review.on_suspicious ?? 'revert'} aria-label="Reviewer action" disabled={busy} onValueChange={(v) => v && change((c) => ({ ...c, review: { ...c.review, on_suspicious: v as 'revert' | 'flag' } }))}>
            <ToggleGroupItem value="revert">Undo</ToggleGroupItem>
            <ToggleGroupItem value="flag">Flag only</ToggleGroupItem>
          </ToggleGroup>
        </Row>
        <Row stack label="Bench lock" description="builtin takes a mkdir slot under /tmp/theuth-bench-slots; or a wrapper command (e.g. ~/.theuth/bin/build-lock) so builds and benchmarks exclude each other.">
          <Input
            key={s?.bench.lock}
            dictate={false}
            aria-label="Bench lock"
            className="w-full max-w-72 font-mono text-[13px]"
            disabled={busy}
            defaultValue={s?.bench.lock}
            onBlur={(e) => {
              const lock = e.currentTarget.value.trim() || 'builtin'
              if (lock !== s?.bench.lock) change((c) => ({ ...c, bench: { ...c.bench, lock } }))
            }}
            onKeyDown={(e) => {
              if (e.key === 'Enter') e.currentTarget.blur()
            }}
          />
        </Row>
        <Row label="Load threshold" description="1-minute load average ÷ cores a scored sample waits for (up to bench.wait); past that it runs and is marked noisy.">
          <Input
            key={s?.bench.max_load}
            type="number"
            dictate={false}
            inputMode="decimal"
            min={0.1}
            max={4}
            step="0.1"
            aria-label="Load threshold"
            className="w-28 text-right"
            disabled={busy}
            defaultValue={s?.bench.max_load}
            onBlur={(e) => {
              const v = Number(e.currentTarget.value)
              if (Number.isFinite(v) && v >= 0.1 && v <= 4 && v !== s?.bench.max_load) change((c) => ({ ...c, bench: { ...c.bench, max_load: v } }))
              else e.currentTarget.value = String(s?.bench.max_load ?? '')
            }}
            onKeyDown={(e) => {
              if (e.key === 'Enter') e.currentTarget.blur()
            }}
          />
        </Row>
        <Row label="Remove worktrees of finished studies after" description="Days after a study finishes before its worktree and sample logs go; the ledger, patches and report stay.">
          <Count value={s?.keep_finished_days} min={1} max={365} action={count((c, keep_finished_days) => ({ ...c, keep_finished_days }))} />
          <span className="text-sm text-fg-secondary">days</span>
        </Row>
      </Section>

      <Mechanism />
    </>
  )
}

/** A USD field edited on blur/Enter; a value that is not a positive number is put back. */
function Money({ value, disabled, onCommit, 'aria-label': ariaLabel }: { value: number | undefined; disabled: boolean; onCommit: (usd: number) => void; 'aria-label': string }) {
  return (
    <>
      <span className="text-sm text-fg-secondary">$</span>
      <Input
        key={value}
        type="number"
        dictate={false}
        inputMode="decimal"
        min={0.01}
        step="0.5"
        aria-label={ariaLabel}
        className="w-24 text-right"
        disabled={disabled || value === undefined}
        defaultValue={value}
        onBlur={(e) => {
          const v = Number(e.currentTarget.value)
          if (Number.isFinite(v) && v > 0 && v !== value) onCommit(Math.round(v * 100) / 100)
          else e.currentTarget.value = String(value ?? '')
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter') e.currentTarget.blur()
        }}
      />
    </>
  )
}

/** A humantime duration on the wire (`600s`), whole seconds in the field (R-99: no rounding to minutes); written back as `<n>s`. */
function Seconds({ value, disabled, onCommit, 'aria-label': ariaLabel }: { value: string | undefined; disabled: boolean; onCommit: (d: string) => void; 'aria-label': string }) {
  const seconds = value === undefined ? undefined : (secondsOf(value) ?? undefined)
  return (
    <>
      <Input
        key={value}
        type="number"
        dictate={false}
        min={1}
        max={14400}
        aria-label={ariaLabel}
        className="w-24 text-right"
        disabled={disabled || value === undefined}
        defaultValue={seconds}
        onBlur={(e) => {
          const n = bounded(e.currentTarget.value, 1, 14400)
          if (n !== undefined && n !== seconds) onCommit(`${n}s`)
          else e.currentTarget.value = String(seconds ?? '')
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter') e.currentTarget.blur()
        }}
      />
      <span className="text-sm text-fg-secondary">s</span>
    </>
  )
}
