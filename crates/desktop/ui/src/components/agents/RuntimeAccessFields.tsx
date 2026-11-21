import { Link } from '@tanstack/react-router'
import { useState } from 'react'
import { ModelCombobox } from '@/components/chat/ModelCombobox'
import { useSandboxes } from '@/components/settings/sandbox-queries'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { cn } from '@/lib/utils'
import type { CompactorProfile, TailMode } from '@/rpc/bindings/method.roster.input'
import { useEndlessSettings, useHarnesses, useMcpServers, useModels } from './queries'
import { type Draft, EFFORTS, TOOLS, tailError } from './wizard'

// Radix Select refuses '' as a value.
const NONE = '__none'

export type Setter = <K extends keyof Draft>(k: K, v: Draft[K]) => void

export function Field({ label, id, hint, error, children }: { label: string; id: string; hint?: React.ReactNode; error?: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5">
      <Label htmlFor={id} className="text-[13px]">
        {label}
      </Label>
      {children}
      {error ? <p className="text-xs text-danger">{error}</p> : hint && <p className="text-xs text-fg-tertiary">{hint}</p>}
    </div>
  )
}

/** Paperclip's "Harness / Runtime" section for a theuth file: model, effort, tools ceiling. Harness is per task, not per agent. */
export function RuntimeFields({ d, set, memoryError }: { d: Draft; set: Setter; memoryError?: string }) {
  const models = useModels()
  const harnesses = useHarnesses()
  const efforts = models.data?.find((m) => m.id === d.model)?.efforts ?? EFFORTS
  const memory = models.data?.find((m) => m.id === d.memory_model)
  const sandboxes = useSandboxes()
  // The named VMs of the list, plus the one on file when it is not (destroyed, or on another hub) so the value stays selectable.
  const pinned = d.runs_in.startsWith('vm:') ? d.runs_in.slice(3) : null
  const vms = [...new Set([...(sandboxes.data ?? []).filter((s) => s.backend === 'vm').map((s) => s.name), ...(pinned ? [pinned] : [])])].sort()
  return (
    <div className="grid gap-4 sm:grid-cols-2">
      <Field label="Model" id="rt-model" hint="Every session this agent opens, and its thinking effort; default = the node's model.">
        <ModelCombobox
          id="rt-model"
          size="sm"
          className="w-full"
          models={models.data ?? []}
          value={d.model}
          none={{ label: 'Node default' }}
          onChange={(v) => set('model', v)}
          effort={{ levels: efforts, current: d.effort, onSelect: (level) => set('effort', level) }}
        />
      </Field>
      <Field label="Context summary model" id="rt-memory-model" hint="Writes the fractal context summaries of its desk, and its thinking effort; a chat's own pick wins. Default = Settings › Context.">
        <ModelCombobox
          id="rt-memory-model"
          aria-label="Context summary model"
          size="sm"
          className="w-full"
          models={models.data ?? []}
          value={d.memory_model}
          none={{ label: 'Use default' }}
          onChange={(v) => {
            set('memory_model', v ?? undefined)
            // 194 D4c/D4d: the effort goes with a cleared model, and with one that does not take it.
            const next = models.data?.find((m) => m.id === v)
            if (d.memory_effort && !next?.efforts.includes(d.memory_effort)) set('memory_effort', undefined)
          }}
          effort={memory && memory.efforts.length > 0 ? { levels: memory.efforts, current: d.memory_effort, onSelect: (level) => set('memory_effort', level ?? undefined) } : undefined}
        />
      </Field>
      <TailFields d={d} set={set} />
      <Field label="Compactor profile" id="rt-profile" hint="How its desk's summaries are written: Small for tiny or local models, Standard otherwise, Automatic picks by model. Default = Settings › Context.">
        <Select value={d.profile ?? NONE} onValueChange={(v) => set('profile', v === NONE ? undefined : (v as CompactorProfile))}>
          <SelectTrigger id="rt-profile" size="sm" className="w-full" aria-label="Compactor profile">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={NONE}>Use default</SelectItem>
            <SelectItem value="auto">Automatic</SelectItem>
            <SelectItem value="standard">Standard</SelectItem>
            <SelectItem value="small">Small</SelectItem>
          </SelectContent>
        </Select>
      </Field>
      <Field label="Tools ceiling" id="rt-tools" hint="The riskiest tool class it and its children may call; inherited downwards.">
        <Select value={d.tools ?? NONE} onValueChange={(v) => set('tools', v === NONE ? null : v)}>
          <SelectTrigger id="rt-tools" size="sm" className="w-full">
            <SelectValue placeholder="Inherit from manager" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={NONE}>Inherit from manager</SelectItem>
            {TOOLS.map((t) => (
              <SelectItem key={t} value={t}>
                {t.replace('_', ' ')}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </Field>
      <Field label="Runs in" id="rt-runs-in" hint={d.runs_in === 'any' ? 'theuth places each task where it fits (67 decision 3).' : d.runs_in === 'vm' ? `Every work session on its own sandbox VM (vm-${d.name || '<name>'}), created on first need.` : 'Every work session on that VM; a stopped VM is started, an unreachable one blocks the task.'}>
        <Select value={d.runs_in} onValueChange={(v) => set('runs_in', v as Draft['runs_in'])}>
          <SelectTrigger id="rt-runs-in" size="sm" className="w-full">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="any">anywhere</SelectItem>
            <SelectItem value="vm">its own VM</SelectItem>
            {vms.map((n) => (
              <SelectItem key={n} value={`vm:${n}`}>
                VM {n}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </Field>
      <div className="sm:col-span-2">
        <div className="flex items-center justify-between gap-4">
          <div>
            <Label htmlFor="rt-isolated-memory" className="text-[13px]">Isolated memory</Label>
            <p id="rt-isolated-help" className="mt-1 text-xs text-fg-secondary">Keeps its own memory. What it learns never reaches your main memory, and it doesn't see yours.</p>
          </div>
          <Switch id="rt-isolated-memory" checked={d.isolated_memory === true} onCheckedChange={(v) => set('isolated_memory', v)} aria-describedby={memoryError ? 'rt-isolated-help rt-isolated-error' : 'rt-isolated-help'} aria-invalid={!!memoryError} />
        </div>
        {memoryError && <p id="rt-isolated-error" role="alert" className="mt-2 text-xs text-danger">{memoryError}</p>}
      </div>
      <div className="flex flex-col gap-1.5">
        <span className="text-[13px] font-medium">Harnesses</span>
        <p className="text-xs text-fg-tertiary">
          An agent is not tied to one harness: theuth picks native or an external harness (
          {harnesses.data?.length ? harnesses.data.map((h) => h.name).join(', ') : 'none configured'}) per task, from what the task needs. Manage them in{' '}
          <Link to="/settings/$tab" params={{ tab: 'harnesses' }} className="text-primary hover:underline">
            Settings › Harnesses
          </Link>
          .
        </p>
      </div>
    </div>
  )
}

/** A tail mark field's text on the wire: KB as whole bytes (R3-12: a fraction makes the agent file unparsable), a
 * percent as a share; `undefined` when empty or not a number. */
export const tailWire = (raw: string, fixed: boolean): number | undefined =>
  raw.trim() === '' || !Number.isFinite(Number(raw)) ? undefined : fixed ? Math.round(Number(raw) * 1000) : Number(raw) / 100

/** 201 §12.1: the agent's own tail sizing — the mode, then that mode's two marks (KB or percent); empty = the default. Only the chosen mode's marks stay on file. */
function TailFields({ d, set }: { d: Draft; set: Setter }) {
  const mode = d.tail_mode ?? null
  const fixed = mode === 'fixed'
  const [lowKey, highKey] = fixed ? (['tail_low_bytes', 'tail_high_bytes'] as const) : (['tail_low_share', 'tail_high_share'] as const)
  const toUi = (v: number | null | undefined) => (v == null ? '' : fixed ? String(v / 1000) : String(Math.round(v * 100)))
  const error = tailError(d, useEndlessSettings().data) ?? undefined
  const unit = fixed ? 'KB' : '%'
  return (
    <>
      <Field label="Context tail" id="rt-tail-mode" hint="How much of its newest chat its desk reads word for word: a fixed size, or a share of the model's window. Default = Settings › Context.">
        <Select
          value={mode ?? NONE}
          onValueChange={(v) => {
            const next = v === NONE ? undefined : (v as TailMode)
            set('tail_mode', next)
            if (next !== 'fixed') {
              set('tail_low_bytes', undefined)
              set('tail_high_bytes', undefined)
            }
            if (next !== 'scaled') {
              set('tail_low_share', undefined)
              set('tail_high_share', undefined)
            }
          }}
        >
          <SelectTrigger id="rt-tail-mode" size="sm" className="w-full" aria-label="Context tail">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={NONE}>Use default</SelectItem>
            <SelectItem value="fixed">Fixed size</SelectItem>
            <SelectItem value="scaled">Scaled to the model</SelectItem>
          </SelectContent>
        </Select>
      </Field>
      {mode && (
        <Field label={`Kept after a trim · Trim at (${unit})`} id="rt-tail-low" hint={fixed ? 'Word for word after a trim, and where a trim is due. 8–256 and 16–512 KB; empty = default.' : "Shares of the model's turn allowance. 2–40 and 5–60 %; empty = default."} error={error}>
          <div className="flex items-center gap-2">
            <Input id="rt-tail-low" type="number" aria-label={`Kept after a trim in ${unit}`} className="h-8" value={toUi(d[lowKey])} onChange={(e) => set(lowKey, tailWire(e.target.value, fixed))} aria-invalid={!!error} />
            <span className="text-fg-tertiary">–</span>
            <Input type="number" aria-label={`Trim at in ${unit}`} className="h-8" value={toUi(d[highKey])} onChange={(e) => set(highKey, tailWire(e.target.value, fixed))} aria-invalid={!!error} />
          </div>
        </Field>
      )}
    </>
  )
}

/** Paperclip's Tools tab: which MCP servers the agent's sessions see. `null` = every server, `[]` = none, else an allowlist. */
export function AccessFields({ d, set }: { d: Draft; set: Setter }) {
  const servers = useMcpServers()
  const mcp = d.mcp ?? []
  // "Only these" with nothing checked yet is still `[]` on the wire; remember the user is picking so the list stays open.
  const [picking, setPicking] = useState(mcp.length > 0)
  const mode = d.mcp === null ? 'all' : picking || mcp.length > 0 ? 'some' : 'none'
  const names = [...new Set([...(servers.data?.map((s) => s.name) ?? []), ...mcp.map((m) => m.replace(/\..*$/, ''))])].sort()
  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-col gap-1.5">
        <ToggleGroup
          type="single"
          variant="outline"
          size="sm"
          value={mode}
          aria-label="MCP servers"
          onValueChange={(v) => {
            if (!v) return
            setPicking(v === 'some')
            set('mcp', v === 'all' ? null : v === 'none' ? [] : mcp)
          }}
        >
          <ToggleGroupItem value="all">All servers</ToggleGroupItem>
          <ToggleGroupItem value="none">None</ToggleGroupItem>
          <ToggleGroupItem value="some">Only these</ToggleGroupItem>
        </ToggleGroup>
        <p className="text-xs text-fg-tertiary">
          {mode === 'all' && 'Sessions see whatever servers this node has connected.'}
          {mode === 'none' && 'Sessions see no MCP tools.'}
          {mode === 'some' && (mcp.length === 0 ? 'Nothing checked yet: until you pick a server, sessions see no MCP tools.' : 'Sessions see only the checked servers.')}
        </p>
      </div>
      {mode === 'some' && (
        <ul className="flex flex-col gap-1">
          {names.length === 0 && <li className="text-sm text-fg-tertiary">No MCP servers are configured on this node yet.</li>}
          {names.map((n) => {
            const on = mcp.some((m) => m === n || m.startsWith(`${n}.`))
            const s = servers.data?.find((x) => x.name === n)
            return (
              <li key={n}>
                <label className={cn('flex items-center gap-3 rounded-lg border border-border-subtle px-3 py-2 text-sm', on && 'bg-layer-1')}>
                  <input
                    type="checkbox"
                    className="accent-primary"
                    checked={on}
                    onChange={(e) => set('mcp', e.target.checked ? [...mcp.filter((m) => m !== n && !m.startsWith(`${n}.`)), n] : mcp.filter((m) => m !== n && !m.startsWith(`${n}.`)))}
                  />
                  <span className="min-w-0 flex-1">
                    <span className="block font-medium">{n}</span>
                    {s && (
                      <span className="block truncate text-xs text-fg-tertiary">
                        {s.tools.length} tools{s.connected ? '' : ' · not connected'}
                      </span>
                    )}
                  </span>
                </label>
              </li>
            )
          })}
        </ul>
      )}
      {mode === 'some' && (
        <Field label="Narrow to tools" id="ac-tools" hint="Optional `server.tool` entries, comma-separated; they replace the whole-server grant for that server.">
          <Input
            id="ac-tools"
            value={mcp.filter((m) => m.includes('.')).join(', ')}
            placeholder="grafana.query, slack.read"
            onChange={(e) => {
              const fine = e.target.value.split(',').map((s) => s.trim()).filter((s) => s.includes('.'))
              const servers = fine.map((f) => f.replace(/\..*$/, ''))
              set('mcp', [...mcp.filter((m) => !m.includes('.') && !servers.includes(m)), ...fine])
            }}
          />
        </Field>
      )}
    </div>
  )
}
