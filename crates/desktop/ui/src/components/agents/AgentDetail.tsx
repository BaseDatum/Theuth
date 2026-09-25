import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { errorText } from '@/components/settings/shared'
import { AGENTS, mockOn } from '@/components/tasks/mock'
import { keys, useAgents } from '@/components/tasks/queries'
import { THEUTH } from '@/components/tasks/helpers'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import { useApi } from '@/lib/rpc'
import type { AgentFile } from '@/rpc/bindings/AgentFile'

const TOOLS = ['read_only', 'mutating', 'destructive'] as const
const EFFORTS = ['low', 'medium', 'high', 'max'] as const

/** The selected agent: `theuth` is a built-in card; anyone else is their file as a form, saved whole with `agent_put`. */
export function AgentDetail({ name }: { name: string }) {
  const api = useApi()
  const file = useQuery({
    queryKey: [...keys.agents, name],
    queryFn: () => (mockOn() ? AGENTS.find((a) => a.file.name === name)?.file ?? Promise.reject(new Error('no such agent')) : api.agent_get(name)),
    enabled: name !== THEUTH,
  })
  if (name === THEUTH) {
    return (
      <div className="mx-auto w-full max-w-[720px] p-4 text-sm">
        <h3 className="text-base font-semibold">theuth</h3>
        <p className="mt-2 text-fg-secondary">
          The built-in default agent (67 decision 1): theuth's own voice, charter <code>prompt.md</code>, reports to you, tools{' '}
          <code>destructive</code>, no MCP limit, no budget. It answers anything unaddressed, owns delegation, and is the root of every
          reporting line. Not a roster entry; edit its charter in <code>prompt.md</code>.
        </p>
      </div>
    )
  }
  if (file.isPending) return <p className="p-4 text-sm text-fg-tertiary">Loading…</p>
  if (file.isError) return <p className="p-4 text-sm text-fg-tertiary">{file.error.message}</p>
  return <Editor key={name} initial={file.data} />
}

function Editor({ initial }: { initial: AgentFile }) {
  const api = useApi()
  const qc = useQueryClient()
  const summary = useAgents().data?.find((a) => a.file.name === initial.name)
  const [f, setF] = useState<AgentFile>(initial)
  const [mcp, setMcp] = useState(initial.mcp.join(', '))
  useEffect(() => {
    setF(initial)
    setMcp(initial.mcp.join(', '))
  }, [initial])
  const set = <K extends keyof AgentFile>(k: K, v: AgentFile[K]) => setF((x) => ({ ...x, [k]: v }))
  const dirty = JSON.stringify(f) !== JSON.stringify(initial) || mcp !== initial.mcp.join(', ')
  const done = (msg: string) => () => {
    void qc.invalidateQueries({ queryKey: keys.agents })
    toast(msg)
  }
  const save = useMutation({
    mutationFn: () => api.agent_put({ ...f, mcp: mcp.split(',').map((s) => s.trim()).filter(Boolean) }),
    onSuccess: done(`Saved ${f.name}`),
    onError: (e) => toast.error(errorText(e)),
  })
  const pause = useMutation({
    mutationFn: (paused: boolean) => api.agent_pause(f.name, paused),
    onSuccess: (_, paused) => done(paused ? `Paused ${f.name}` : `Resumed ${f.name}`)(),
    onError: (e) => toast.error(errorText(e)),
  })
  const approve = useMutation({
    mutationFn: () => api.agent_approve(f.name),
    onSuccess: done(`Approved ${f.name}`),
    onError: (e) => toast.error(errorText(e)),
  })
  const busy = save.isPending || pause.isPending || approve.isPending
  const proposed = summary?.status === 'proposed' || !!initial.proposed_by

  return (
    <form
      className="mx-auto flex w-full max-w-[720px] flex-col gap-4 p-4"
      onSubmit={(e) => {
        e.preventDefault()
        save.mutate()
      }}
    >
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="min-w-0 flex-1 truncate text-base font-semibold">
          {initial.title} <span className="font-mono text-sm text-fg-tertiary">{initial.name}</span>
        </h3>
        {summary && <Badge variant="outline">{summary.status}</Badge>}
        {proposed ? (
          <>
            <span className="text-[13px] text-fg-secondary">proposed by {initial.proposed_by ?? '—'}</span>
            <Button type="button" size="sm" disabled={busy} onClick={() => approve.mutate()}>
              Approve
            </Button>
          </>
        ) : (
          <Button type="button" size="sm" variant="outline" disabled={busy} onClick={() => pause.mutate(!initial.paused)}>
            {initial.paused ? 'Resume' : 'Pause'}
          </Button>
        )}
      </div>
      <div className="grid gap-3 sm:grid-cols-2">
        <Field label="Title" id="af-title">
          <Input id="af-title" value={f.title} onChange={(e) => set('title', e.target.value)} />
        </Field>
        <Field label="Reports to" id="af-reports" hint="an agent name, or theuth">
          <Input id="af-reports" value={f.reports_to} onChange={(e) => set('reports_to', e.target.value)} />
        </Field>
        <Field label="Model" id="af-model" hint="optional">
          <Input id="af-model" value={f.model ?? ''} placeholder="provider/model" onChange={(e) => set('model', e.target.value || null)} />
        </Field>
        <Field label="Effort" id="af-effort" hint="optional">
          <NativeSelect id="af-effort" value={f.effort ?? ''} onChange={(v) => set('effort', v || null)} options={EFFORTS} none="default" />
        </Field>
        <Field label="Tools" id="af-tools" hint="risk ceiling">
          <NativeSelect id="af-tools" value={f.tools ?? ''} onChange={(v) => set('tools', v || null)} options={TOOLS} none="inherit" />
        </Field>
        <Field label="MCP" id="af-mcp" hint="servers or server.tool, comma-separated; empty = all">
          <Input id="af-mcp" value={mcp} onChange={(e) => setMcp(e.target.value)} />
        </Field>
        <Field label="Budget" id="af-budget" hint="per month; none = infinite">
          <div className="flex items-center gap-2">
            <NativeSelect
              id="af-budget"
              value={f.budget.type}
              onChange={(v) => set('budget', v === 'none' ? { type: 'none' } : { type: 'monthly', amount: 0, currency: 'USD' })}
              options={['none', 'monthly']}
            />
            {f.budget.type === 'monthly' && (
              <>
                <Input
                  type="number"
                  min={0}
                  step="any"
                  className="w-24"
                  aria-label="Budget amount"
                  value={f.budget.amount}
                  onChange={(e) => set('budget', { type: 'monthly', amount: Number(e.target.value), currency: f.budget.type === 'monthly' ? f.budget.currency : 'USD' })}
                />
                <Input
                  className="w-20 uppercase"
                  aria-label="Currency"
                  maxLength={3}
                  value={f.budget.currency}
                  onChange={(e) => set('budget', { type: 'monthly', amount: f.budget.type === 'monthly' ? f.budget.amount : 0, currency: e.target.value.toUpperCase() })}
                />
              </>
            )}
          </div>
        </Field>
        <Field label="Paused" id="af-paused">
          <Switch id="af-paused" checked={f.paused} onCheckedChange={(v) => set('paused', v)} />
        </Field>
      </div>
      <Field label="Capabilities" id="af-cap" hint="when I am relevant — read by other agents when handing off">
        <Textarea id="af-cap" rows={2} value={f.capabilities ?? ''} onChange={(e) => set('capabilities', e.target.value || null)} />
      </Field>
      <Field label="Charter" id="af-body" hint="markdown body; the desk's system prompt is built from this">
        <Textarea id="af-body" rows={14} className="font-mono text-[13px]" value={f.body} onChange={(e) => set('body', e.target.value)} />
      </Field>
      {summary && (
        <p className="text-[13px] text-fg-tertiary">
          {summary.open_tasks} open tasks · spent {summary.spent_month}
          {f.budget.type === 'monthly' && ` / ${f.budget.amount} ${f.budget.currency}`} this month
          {summary.desk && ` · desk ${summary.desk}`}
        </p>
      )}
      <div className="flex justify-end gap-2">
        <Button type="button" variant="ghost" size="sm" disabled={!dirty || busy} onClick={() => { setF(initial); setMcp(initial.mcp.join(', ')) }}>
          Revert
        </Button>
        <Button type="submit" size="sm" disabled={!dirty || busy || !f.body.trim()}>
          Save
        </Button>
      </div>
    </form>
  )
}

export function Field({ label, id, hint, children }: { label: string; id: string; hint?: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5">
      <Label htmlFor={id} className="text-[13px] text-fg-secondary">
        {label}
        {hint && <span className="font-normal text-fg-tertiary"> · {hint}</span>}
      </Label>
      {children}
    </div>
  )
}

export function NativeSelect({ id, value, onChange, options, none }: { id: string; value: string; onChange(v: string): void; options: readonly string[]; none?: string }) {
  return (
    <select id={id} className="h-8 rounded-md border border-input bg-transparent px-2 text-sm" value={value} onChange={(e) => onChange(e.target.value)}>
      {none !== undefined && <option value="">{none}</option>}
      {options.map((o) => (
        <option key={o} value={o}>
          {o}
        </option>
      ))}
    </select>
  )
}
