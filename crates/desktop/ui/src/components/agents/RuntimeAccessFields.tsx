import { Link } from '@tanstack/react-router'
import { useState } from 'react'
import { ModelCombobox } from '@/components/chat/ModelCombobox'
import { useSandboxes } from '@/components/settings/sandbox-queries'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { cn } from '@/lib/utils'
import { useHarnesses, useMcpServers, useModels } from './queries'
import { type Draft, EFFORTS, TOOLS } from './wizard'

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
export function RuntimeFields({ d, set }: { d: Draft; set: Setter }) {
  const models = useModels()
  const harnesses = useHarnesses()
  const efforts = models.data?.find((m) => m.id === d.model)?.efforts ?? EFFORTS
  const sandboxes = useSandboxes()
  // The named VMs of the list, plus the one on file when it is not (destroyed, or on another hub) so the value stays selectable.
  const pinned = d.runs_in.startsWith('vm:') ? d.runs_in.slice(3) : null
  const vms = [...new Set([...(sandboxes.data ?? []).filter((s) => s.backend === 'vm').map((s) => s.name), ...(pinned ? [pinned] : [])])].sort()
  return (
    <div className="grid gap-4 sm:grid-cols-2">
      <Field label="Model" id="rt-model" hint="Every session this agent opens; default = the node's model.">
        <ModelCombobox id="rt-model" size="sm" className="w-full" models={models.data ?? []} value={d.model} none={{ label: 'Node default' }} onChange={(v) => set('model', v)} />
      </Field>
      <Field label="Memory summary model" id="rt-memory-model" hint="Writes the endless memory summaries of its desk; a chat's own pick wins. Default = Settings › Memory.">
        <ModelCombobox
          id="rt-memory-model"
          aria-label="Memory summary model"
          size="sm"
          className="w-full"
          models={models.data ?? []}
          value={d.memory_model}
          none={{ label: 'Use default' }}
          onChange={(v) => set('memory_model', v ?? undefined)}
        />
      </Field>
      <Field label="Thinking effort" id="rt-effort" hint="Reasoning depth for its sessions.">
        <Select value={d.effort ?? NONE} onValueChange={(v) => set('effort', v === NONE ? null : v)}>
          <SelectTrigger id="rt-effort" size="sm" className="w-full">
            <SelectValue placeholder="Default" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={NONE}>Default</SelectItem>
            {efforts.map((e) => (
              <SelectItem key={e} value={e}>
                {e}
              </SelectItem>
            ))}
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
      <div className="flex flex-col gap-1.5">
        <span className="text-[13px] font-medium">Harnesses</span>
        <p className="text-xs text-fg-tertiary">
          An agent is not tied to one harness: theuth picks native or an external harness (
          {harnesses.data?.length ? harnesses.data.map((h) => h.name).join(', ') : 'none configured'}) per task, from what the task needs. Manage them in{' '}
          <Link to="/settings/$tab" params={{ tab: 'agents' }} className="text-primary hover:underline">
            Settings › Harnesses
          </Link>
          .
        </p>
      </div>
    </div>
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
