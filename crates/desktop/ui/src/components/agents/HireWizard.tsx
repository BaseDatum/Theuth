import { Link, useNavigate } from '@tanstack/react-router'
import { ChevronLeft, PanelLeft } from 'lucide-react'
import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { SearchTrigger } from '@/components/sidebar/SearchTrigger'
import { useAgents } from '@/components/tasks/queries'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { useSidebar } from '@/components/ui/sidebar'
import { Textarea } from '@/components/ui/textarea'
import { isMacDesktop } from '@/lib/hotkeys'
import { useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { CharterDoc, useCharterDoc } from './CharterDoc'
import { AccessFields, Field, RuntimeFields, type Setter } from './RuntimeAccessFields'
import { AgentAvatar } from './identity'
import { managerNames } from './hiring'
import { useAgentActions } from './queries'
import {
  type Draft,
  STEPS,
  STEP_TITLE,
  type Step,
  clearDraft,
  emptyDraft,
  firstInvalid,
  loadDraft,
  saveDraft,
  starterCharter,
  suggestName,
  toFile,
  validate,
  withCharter,
} from './wizard'

const LABEL: Record<Step, string> = { identity: 'Identity', role: 'Role', runtime: 'Runtime', access: 'Access', budget: 'Budget', charter: 'Charter', review: 'Review' }

/**
 * `/agents/new` — paperclip's hire flow (AgentBasicsDialog + NewAgentSetup) as one stepped page. Each step validates
 * before Next; the draft and the step live in localStorage so a reload resumes in place. The charter is a document
 * (`agent-charter:<name>`, seeded with the starter template on the Charter step); Next and Hire copy its markdown into
 * `body`, and Hire writes the file with `agent_put` (unpaused, or paused for "Hire paused") and lands on the new
 * agent's overview — from then on the file is the source of truth and the document follows it.
 */
export function HireWizard() {
  const sidebar = useSidebar()
  const mac = isMacDesktop()
  const navigate = useNavigate()
  const agents = useAgents()
  const { put } = useAgentActions()
  const api = useApi()
  const [saved] = useState(loadDraft)
  const [step, setStep] = useState<Step>(saved?.step ?? 'identity')
  const [d, setD] = useState<Draft>(saved?.draft ?? emptyDraft)
  const [touched, setTouched] = useState(false)
  const taken = agents.data?.map((a) => a.file.name) ?? []
  const errors = validate(step, d, taken)
  const set: Setter = (k, v) => setD((x) => ({ ...x, [k]: v }))
  useEffect(() => saveDraft({ step, draft: d }), [step, d])

  const at = STEPS.indexOf(step)
  // Forward jumps stop at the first incomplete step; back is always allowed.
  const bad = firstInvalid(d, taken)
  const reachable = bad === null ? STEPS.length : STEPS.indexOf(bad)
  const go = (s: Step) => {
    setTouched(false)
    setStep(s)
  }
  // The charter document exists from the Charter step on (an empty name disables the query), so renaming on
  // Identity never mints documents for names that were only passing through.
  const charter = useCharterDoc(step === 'charter' || step === 'review' ? d.name : '', starterCharter(d))
  // The draft with the document's current markdown as `body` (what Hire writes and Review shows).
  const fromDoc = async (): Promise<Draft> => {
    try {
      const draft = await withCharter(d, charter.data ?? null, (doc) => api.doc_markdown(doc))
      setD(draft)
      return draft
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
      return d
    }
  }
  const next = async () => {
    const draft = step === 'charter' ? await fromDoc() : d
    if (Object.keys(validate(step, draft, taken)).length > 0) return setTouched(true)
    go(STEPS[at + 1] ?? 'review')
  }
  const hire = async (paused: boolean) => {
    const draft = await fromDoc()
    const bad = firstInvalid(draft, taken)
    if (bad) return go(bad)
    put.mutate(toFile(draft, paused), {
      onSuccess: () => {
        clearDraft()
        void navigate({ to: '/agents/$name/$tab', params: { name: draft.name, tab: 'overview' } })
      },
    })
  }
  const err = (k: keyof Draft) => (touched ? errors[k] : undefined)

  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header
        data-tauri-drag-region={mac || undefined}
        className={cn('flex h-[52px] shrink-0 items-center gap-1 border-b border-border-subtle px-3', !sidebar.open && mac && 'pl-20')}
      >
        {(!sidebar.open || sidebar.isMobile) && (
          <div className="flex shrink-0 items-center">
            <SearchTrigger />
            <Button variant="ghost" size="icon-sm" aria-label="Show sidebar" onClick={sidebar.toggleSidebar}>
              <PanelLeft />
            </Button>
          </div>
        )}
        <Button asChild variant="ghost" size="icon-sm" aria-label="Back to agents">
          <Link to="/agents">
            <ChevronLeft />
          </Link>
        </Button>
        <h2 className="min-w-0 flex-1 truncate text-sm font-semibold">
          <span className="text-fg-tertiary">Agents › </span>Hire an agent
        </h2>
        {saved && (
          <Button
            variant="ghost"
            size="sm"
            onClick={() => {
              clearDraft()
              setD(emptyDraft())
              go('identity')
            }}
          >
            Start over
          </Button>
        )}
      </header>
      <div className="flex min-h-0 flex-1 flex-col md:flex-row">
        <nav aria-label="Hire steps" className="shrink-0 border-b border-border-subtle p-3 md:w-52 md:border-r md:border-b-0">
          <ol className="flex gap-1 overflow-x-auto md:flex-col">
            {STEPS.map((s, i) => {
              const complete = i < at && Object.keys(validate(s, d, taken)).length === 0
              return (
                <li key={s}>
                  <button
                    type="button"
                    disabled={i > at && i > reachable}
                    onClick={() => go(s)}
                    className={cn(
                      'flex w-full items-center gap-2 rounded-[10px] px-2.5 py-1.5 text-left text-[13px] hover:bg-layer-2 disabled:opacity-50 disabled:hover:bg-transparent',
                      s === step && 'bg-layer-2 font-medium',
                    )}
                  >
                    <span className={cn('flex size-5 shrink-0 items-center justify-center rounded-full text-[11px] tabular-nums', complete ? 'bg-primary text-primary-foreground' : 'bg-layer-3 text-fg-secondary')}>
                      {i + 1}
                    </span>
                    {LABEL[s]}
                  </button>
                </li>
              )
            })}
          </ol>
        </nav>
        <main className="flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto">
          <div className={cn('mx-auto flex w-full flex-1 flex-col gap-5 p-4 md:p-6', step === 'charter' ? 'max-w-[880px]' : 'max-w-[640px]')}>
            <div>
              <p className="text-xs text-fg-tertiary">
                Step {at + 1} of {STEPS.length}
              </p>
              <h3 className="text-lg font-semibold">{STEP_TITLE[step]}</h3>
            </div>
            <StepBody step={step} d={d} set={set} err={err} taken={taken} />
          </div>
          <footer className="sticky bottom-0 flex shrink-0 items-center gap-2 border-t border-border-subtle bg-background px-4 py-3">
            {touched && Object.keys(errors).length > 0 && <p className="text-[13px] text-danger">Fix the highlighted fields to continue.</p>}
            <span className="flex-1" />
            <Button variant="ghost" size="sm" disabled={at === 0} onClick={() => go(STEPS[at - 1] ?? 'identity')}>
              Back
            </Button>
            {step !== 'review' ? (
              <Button size="sm" onClick={() => void next()}>
                Next
              </Button>
            ) : (
              <>
                <Button size="sm" variant="outline" disabled={put.isPending} onClick={() => void hire(true)}>
                  Hire paused
                </Button>
                <Button size="sm" disabled={put.isPending} onClick={() => void hire(false)}>
                  Hire {d.title.trim() || d.name}
                </Button>
              </>
            )}
          </footer>
        </main>
      </div>
    </article>
  )
}

function StepBody({ step, d, set, err, taken }: { step: Step; d: Draft; set: Setter; err: (k: keyof Draft) => string | undefined; taken: string[] }) {
  const agents = useAgents()
  switch (step) {
    case 'identity':
      return (
        <div className="flex flex-col gap-4">
          <p className="text-sm text-fg-secondary">Start with a name. Make them your own.</p>
          <div className="flex items-center gap-4">
            <AgentAvatar name={d.name || 'new'} title={d.title || d.name} size={56} />
            <div className="grid flex-1 gap-4 sm:grid-cols-2">
              <Field label="Title" id="id-title" hint="Shown on the org chart, e.g. Chief of staff." error={err('title')}>
                <Input
                  id="id-title"
                  autoFocus
                  value={d.title}
                  placeholder="e.g. Chief of staff"
                  onChange={(e) => {
                    const t = e.target.value
                    set('title', t)
                    if (!d.name || d.name === suggestName(d.title)) set('name', suggestName(t))
                  }}
                />
              </Field>
              <Field label="Name" id="id-name" hint="The file agents/<name>.md and how others address it: lowercase, digits, dashes." error={err('name')}>
                <Input id="id-name" className="font-mono" value={d.name} placeholder="chief-of-staff" maxLength={32} onChange={(e) => set('name', e.target.value.toLowerCase())} />
              </Field>
            </div>
          </div>
          <p className="text-xs text-fg-tertiary">The avatar is its initials on a colour derived from the name.</p>
        </div>
      )
    case 'role':
      return (
        <div className="flex flex-col gap-4">
          <Field label="Reports to" id="ro-manager" hint="Its manager in the org chart. Escalations and budget notices go up this line." error={err('reports_to')}>
            <Select value={d.reports_to} onValueChange={(v) => set('reports_to', v)}>
              <SelectTrigger id="ro-manager" size="sm" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {managerNames(agents.data, d.name).map((n) => (
                  <SelectItem key={n} value={n}>
                    {n === 'theuth' ? 'theuth (built-in root)' : `${agents.data?.find((a) => a.file.name === n)?.file.title ?? n} · ${n}`}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </Field>
          <Field label="Capabilities" id="ro-cap" hint="When to hand off to this agent. Read by other agents when routing work; shown on the org chart.">
            <Textarea id="ro-cap" rows={3} value={d.capabilities ?? ''} placeholder="e.g. anything about production incidents, on-call and postmortems" onChange={(e) => set('capabilities', e.target.value || null)} />
          </Field>
          {taken.length === 0 && <p className="text-xs text-fg-tertiary">The roster has no other agents yet, so this one reports to theuth.</p>}
        </div>
      )
    case 'runtime':
      return <RuntimeFields d={d} set={set} />
    case 'access':
      return <AccessFields d={d} set={set} />
    case 'budget':
      return <BudgetStep d={d} set={set} error={err('budget')} />
    case 'charter':
      return (
        <div className="flex min-h-[560px] flex-1 flex-col gap-2">
          <p className="text-sm text-fg-secondary">
            The charter is the desk's system prompt: what it owns, how it works, when to escalate. Write it here or with theuth in the chat below.
          </p>
          <CharterDoc who={d} seed={starterCharter(d)} hint={`Hire writes this into agents/${d.name}.md.`} className="min-h-[520px] rounded-lg border border-border-subtle" />
          {err('body') && <p className="text-xs text-danger">{err('body')}</p>}
        </div>
      )
    case 'review':
      return <Review d={d} />
    default:
      return null
  }
}

function BudgetStep({ d, set, error }: { d: Draft; set: Setter; error?: string }) {
  const monthly = d.budget.type === 'monthly'
  return (
    <div className="flex flex-col gap-4">
      <div className="grid gap-2 sm:grid-cols-2">
        <button
          type="button"
          onClick={() => set('budget', { type: 'none' })}
          className={cn('rounded-lg border p-3 text-left text-sm hover:bg-layer-2', !monthly ? 'border-primary bg-layer-1' : 'border-border-subtle')}
        >
          <span className="block font-medium">No budget</span>
          <span className="block text-xs text-fg-tertiary">Spend is counted but never capped.</span>
        </button>
        <button
          type="button"
          onClick={() => !monthly && set('budget', { type: 'monthly', amount: 20, currency: 'USD' })}
          className={cn('rounded-lg border p-3 text-left text-sm hover:bg-layer-2', monthly ? 'border-primary bg-layer-1' : 'border-border-subtle')}
        >
          <span className="block font-medium">Monthly budget</span>
          <span className="block text-xs text-fg-tertiary">Soft alert at 80 %, hard stop at 100 %.</span>
        </button>
      </div>
      {d.budget.type === 'monthly' && (
        <Field label="Budget per calendar month" id="bu-amount" error={error} hint="Billed model spend across every session this agent opens, in the budget's currency.">
          <div className="flex items-center gap-2">
            <Input
              id="bu-amount"
              type="number"
              min={0}
              step="any"
              className="w-32"
              value={d.budget.amount}
              onChange={(e) => set('budget', { type: 'monthly', amount: Number(e.target.value), currency: d.budget.type === 'monthly' ? d.budget.currency : 'USD' })}
            />
            <Input
              className="w-20 uppercase"
              aria-label="Currency"
              maxLength={3}
              value={d.budget.currency}
              onChange={(e) => set('budget', { type: 'monthly', amount: d.budget.type === 'monthly' ? d.budget.amount : 0, currency: e.target.value.toUpperCase() })}
            />
          </div>
        </Field>
      )}
      <p className="rounded-lg border border-border-subtle bg-layer-1 p-3 text-[13px] text-fg-secondary">
        {monthly ? (
          <>
            At 80 % of the limit its manager gets a note. At 100 % the agent is <b>paused</b> and queued work is held until you resume it — resuming lifts the cap for the rest
            of the month.
          </>
        ) : (
          <>No cap configured. You can set one later from the agent's Overview.</>
        )}
      </p>
    </div>
  )
}

function Review({ d }: { d: Draft }) {
  const rows: [string, React.ReactNode][] = [
    ['Name', <code key="n">{d.name}</code>],
    ['Title', d.title],
    ['Reports to', d.reports_to],
    ['Capabilities', d.capabilities?.trim() || <span className="text-fg-tertiary">none</span>],
    ['Model', d.model ?? <span className="text-fg-tertiary">node default</span>],
    ['Effort', d.effort ?? <span className="text-fg-tertiary">default</span>],
    ['Tools ceiling', d.tools ?? <span className="text-fg-tertiary">inherited</span>],
    ['MCP', d.mcp === null ? <span className="text-fg-tertiary">every server</span> : d.mcp.length === 0 ? <span className="text-fg-tertiary">none</span> : d.mcp.join(', ')],
    ['Budget', d.budget.type === 'none' ? <span className="text-fg-tertiary">none</span> : `${d.budget.amount} ${d.budget.currency} / month`],
  ]
  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-center gap-3">
        <AgentAvatar name={d.name} title={d.title} size={48} />
        <div>
          <p className="font-semibold">{d.title}</p>
          <p className="font-mono text-xs text-fg-tertiary">agents/{d.name}.md</p>
        </div>
      </div>
      <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-2 rounded-lg border border-border-subtle p-3 text-sm">
        {rows.map(([k, v]) => (
          <div key={k} className="contents">
            <dt className="text-fg-tertiary">{k}</dt>
            <dd className="min-w-0 break-words">{v}</dd>
          </div>
        ))}
      </dl>
      <details className="rounded-lg border border-border-subtle p-3 text-sm">
        <summary className="cursor-pointer font-medium">Charter</summary>
        <pre className="mt-2 whitespace-pre-wrap font-mono text-xs text-fg-secondary">{d.body}</pre>
      </details>
      <p className="text-[13px] text-fg-secondary">
        <b>Hire</b> writes the file and opens its desk on the next wake. <b>Hire paused</b> writes it with <code>paused: true</code> so you can finish the charter first.
      </p>
    </div>
  )
}
