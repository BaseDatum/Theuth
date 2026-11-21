import { THEUTH } from '@/components/tasks/helpers'
import type { AgentInput as AgentFile } from '@/lib/agent-memory'
import { ENDLESS_DEFAULTS, tailMark, tailPairError } from '@/lib/endless'
import type { EndlessConfig } from '@/rpc/bindings/method.agent.output'

/** `/agents/new` in paperclip's order: who → where in the org → how it runs → what it may reach → what it may spend → its charter → review. */
export const STEPS = ['identity', 'role', 'runtime', 'access', 'budget', 'charter', 'review'] as const
export type Step = (typeof STEPS)[number]
export const STEP_TITLE: Record<Step, string> = {
  identity: 'Meet your next agent',
  role: 'Where does it sit?',
  runtime: 'How does it run?',
  access: 'What may it reach?',
  budget: 'What may it spend?',
  charter: 'Its charter',
  review: 'Review and hire',
}

/** The file minus `paused` / `proposed_by`, which the Hire buttons decide. */
export type Draft = Omit<AgentFile, 'paused' | 'proposed_by'> & Required<Pick<AgentFile, 'runs_in' | 'capabilities' | 'mcp'>>

export const NAME_RE = /^[a-z0-9-]{1,32}$/
export const TOOLS = ['read_only', 'mutating', 'destructive'] as const
export const EFFORTS = ['low', 'medium', 'high', 'max'] as const

export function emptyDraft(): Draft {
  return { name: '', title: '', reports_to: THEUTH, model: null, effort: null, tools: null, mcp: null, budget: { type: 'none' }, capabilities: null, runs_in: 'any', isolated_memory: false, body: '' }
}

/** The charter the editor opens with; the agent fills in the blanks. */
export function starterCharter(d: Draft): string {
  const who = d.title.trim() || 'this agent'
  return `# ${who}

You are ${who}${d.reports_to ? `, reporting to ${d.reports_to}` : ''}.

## What you own

- 

## How you work

- Work only through tasks: check one out, do it in a child session, hand the result back for review.
- Escalate decisions you may not make yourself instead of guessing.
${d.capabilities?.trim() ? `\n## When to hand off to you\n\n${d.capabilities.trim()}\n` : ''}`
}

/** A `title` slugged into a candidate `name`. */
export function suggestName(title: string): string {
  return title
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 32)
}

/** Field errors for one step; empty = the step is complete. `taken` is every roster name; `tails` is Settings › Context
 * (`endless_settings_get`), which a lone per-agent tail mark pairs with (engine defaults while unknown). */
export function validate(step: Step, d: Draft, taken: readonly string[], tails?: TailDefaults | null): Partial<Record<keyof Draft, string>> {
  const e: Partial<Record<keyof Draft, string>> = {}
  switch (step) {
    case 'identity':
      if (!NAME_RE.test(d.name)) e.name = 'lowercase letters, digits and dashes; 1–32 characters'
      else if (d.name === THEUTH) e.name = 'theuth is the built-in root'
      else if (taken.includes(d.name)) e.name = 'that name is on the roster already'
      if (!d.title.trim()) e.title = 'a title is required'
      break
    case 'role':
      if (d.reports_to !== THEUTH && !taken.includes(d.reports_to)) e.reports_to = 'pick a manager from the roster'
      else if (d.reports_to === d.name) e.reports_to = 'an agent cannot report to itself'
      break
    case 'budget':
      if (d.budget.type === 'monthly') {
        if (!(d.budget.amount > 0)) e.budget = 'enter an amount above zero, or choose no budget'
        else if (!/^[A-Z]{3}$/.test(d.budget.currency)) e.budget = 'currency is a three-letter code, e.g. USD'
      }
      break
    case 'runtime': {
      const tail = tailError(d, tails)
      if (tail) e.tail_mode = tail
      break
    }
    case 'charter':
      if (!d.body.trim()) e.body = 'the charter is the desk\u2019s system prompt; it cannot be empty'
      break
    case 'review':
      for (const s of STEPS) if (s !== 'review') Object.assign(e, validate(s, d, taken, tails))
      break
    default:
  }
  return e
}

/** The first step with an error, or `null` when the whole draft is hireable. */
export function firstInvalid(d: Draft, taken: readonly string[], tails?: TailDefaults | null): Step | null {
  return STEPS.find((s) => s !== 'review' && Object.keys(validate(s, d, taken, tails)).length > 0) ?? null
}

/** 201 R3-12: Settings › Context's tail marks, the other half of a pair the agent file gives only one mark of. */
export type TailDefaults = Partial<Pick<EndlessConfig, 'tail_low_bytes' | 'tail_high_bytes' | 'tail_low_share' | 'tail_high_share'>>

/** 201 §12.1: a per-agent tail pair must be in range with the high mark above the low one (roster `file::parse` refuses
 * it otherwise). R3-12: a lone mark is checked against `tails`' other mark (engine defaults while unknown), since the
 * engine pairs it with that one; the reason then names the setting it met. */
export function tailError(d: Pick<Draft, 'tail_mode' | 'tail_low_bytes' | 'tail_high_bytes' | 'tail_low_share' | 'tail_high_share'>, tails?: TailDefaults | null): string | null {
  if (!d.tail_mode) return null
  const fixed = d.tail_mode === 'fixed'
  const [low, high] = fixed ? [d.tail_low_bytes, d.tail_high_bytes] : [d.tail_low_share, d.tail_high_share]
  if (low == null && high == null) return null
  const mark = <K extends keyof TailDefaults>(k: K): number => tails?.[k] ?? ENDLESS_DEFAULTS[k]
  const [gLow, gHigh] = fixed ? [mark('tail_low_bytes'), mark('tail_high_bytes')] : [mark('tail_low_share'), mark('tail_high_share')]
  const err = tailPairError(d.tail_mode, low ?? gLow, high ?? gHigh)
  // Only the order can fail because of the inherited mark; a lone mark out of its own range reads as it is.
  if (err === null || (low != null && high != null) || !err.startsWith('Trim at (')) return err
  return low == null
    ? `${err} Kept after a trim is empty, so it is Settings › Context's ${tailMark(d.tail_mode, gLow)}; set it here too.`
    : `${err} Trim at is empty, so it is Settings › Context's ${tailMark(d.tail_mode, gHigh)}; set it here too.`
}

export function toFile(d: Draft, paused: boolean): AgentFile {
  return {
    ...d,
    title: d.title.trim(),
    capabilities: d.capabilities?.trim() || null,
    model: d.model || null,
    effort: d.effort || null,
    tools: d.tools || null,
    paused,
    proposed_by: null,
  }
}

/**
 * The draft with the charter document's markdown as `body` — what Hire writes into `agents/<name>.md` and Review
 * shows. Without a document (the query has not resolved yet) the draft is unchanged.
 */
export async function withCharter(d: Draft, doc: string | null, markdown: (doc: string) => Promise<string>): Promise<Draft> {
  return doc ? { ...d, body: (await markdown(doc)).trim() } : d
}

/** Resume in place (paperclip keeps its wizard in localStorage): the draft and the step survive a reload. */
export const DRAFT_KEY = 'theuth.hire-draft'
export type Saved = { step: Step; draft: Draft }

export function loadDraft(): Saved | null {
  try {
    const raw = localStorage.getItem(DRAFT_KEY)
    if (!raw) return null
    const s = JSON.parse(raw) as Partial<Saved>
    if (!s.draft || !STEPS.includes(s.step as Step)) return null
    return { step: s.step as Step, draft: { ...emptyDraft(), ...s.draft } }
  } catch {
    return null
  }
}

export function saveDraft(s: Saved): void {
  try {
    localStorage.setItem(DRAFT_KEY, JSON.stringify(s))
  } catch {
    /* storage unavailable: the wizard still works within the page */
  }
}

export function clearDraft(): void {
  try {
    localStorage.removeItem(DRAFT_KEY)
  } catch {
    /* ignore */
  }
}
