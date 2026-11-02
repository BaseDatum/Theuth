// 160 `?sim=costs`: a synthetic, deterministic cost cube (~8 weeks ending at `now`), its ledger pages and a usage-limits
// snapshot with a row in every status. Shaped like a real ledger (one dominant model, an endless compactor, a reported
// omp leg, a deleted root, unpriced failures) — every title, name and number here is invented.

import type { CostCall } from '@/rpc/bindings/CostCall'
import type { CostCallsPage } from '@/rpc/bindings/CostCallsPage'
import type { CostCallsQuery } from '@/rpc/bindings/CostCallsQuery'
import type { CostCells } from '@/rpc/bindings/CostCells'
import type { CostCube } from '@/rpc/bindings/CostCube'
import type { CostRootEntry } from '@/rpc/bindings/CostRootEntry'
import type { UsageLimits } from '@/rpc/bindings/UsageLimits'
import { BASES } from '@/lib/costs/cube'

const HOUR = 3_600_000

/** mulberry32: the same stream for the same seed. */
function rng(seed: number): () => number {
  let a = seed >>> 0
  return () => {
    a = (a + 0x6d2b79f5) >>> 0
    let t = a
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

const PROVIDERS = ['anthropic', 'openai', 'amazon-bedrock']
const KINDS = ['round', 'endless', 'harvest', 'title', 'dream', 'escalate', 'harness']
const VIAS = ['theuth', 'omp', 'opencode']
/** [id, name, provider ix, $/M input, $/M output, $/M cache read] */
const MODELS: [string, string, number, number, number, number][] = [
  ['anthropic/claude-fable-5-1', 'claude-fable-5-1', 0, 5, 25, 0.5],
  ['anthropic/claude-opus-5-5', 'claude-opus-5-5', 0, 5, 25, 0.5],
  ['anthropic/claude-haiku-4-5', 'claude-haiku-4-5', 0, 1, 5, 0.1],
  ['anthropic/claude-haiku-4-5-20251001', 'claude-haiku-4-5', 0, 1, 5, 0.1],
  ['anthropic/claude-sonnet-4-5', 'claude-sonnet-4-5', 0, 3, 15, 0.3],
  ['openai/gpt-6-astra', 'gpt-6-astra', 1, 2.5, 15, 0.25],
  ['amazon-bedrock/us.openai.gpt-6-astra', 'gpt-6-astra', 2, 2.5, 15, 0.25],
]

const TITLES = [
  'Plan the spring release notes',
  'Refactor the cache eviction path',
  'Draft the onboarding guide',
  'Investigate flaky upload test',
  'Sketch the pricing page',
  'Tidy the build scripts',
  'Review the sync protocol design',
  'Prototype offline mode',
  'Write the migration runbook',
  'Profile the search indexer',
  'Polish the settings layout',
  'Explore a plugin API',
]

type Root = { title: string; owner_kind: 'agent' | 'user' | 'routine' | 'other'; owner: string; project: string | null; deleted?: boolean; weight: number; via?: number }

const ROOTS: Root[] = [
  ...TITLES.map((title, i): Root => ({ title, owner_kind: 'user', owner: 'you', project: i % 3 === 0 ? 'atlas' : i % 3 === 1 ? 'harbor' : null, weight: 1 + (i % 4) })),
  { title: 'Desk · theuth', owner_kind: 'agent', owner: 'theuth', project: 'atlas', weight: 9 },
  { title: 'Desk · ops', owner_kind: 'agent', owner: 'ops-desk', project: 'harbor', weight: 3 },
  { title: 'dreaming · night', owner_kind: 'routine', owner: 'dreaming', project: null, weight: 2 },
  { title: 'morning-brief · daily', owner_kind: 'routine', owner: 'morning-brief', project: null, weight: 1 },
  { title: 'Port the CLI to the new runner', owner_kind: 'user', owner: 'you', project: 'harbor', weight: 4, via: 1 },
  { title: 'Opencode trial run', owner_kind: 'user', owner: 'you', project: null, weight: 1, via: 2 },
  { title: '', owner_kind: 'user', owner: 'you', project: 'atlas', deleted: true, weight: 2 },
]

export const COSTS_FIXTURE_ENGINE = 'sim-engine'

/** The fixture cube: 56 days of hours ending at `now`'s hour. */
export function costsFixture(now: number): CostCube {
  const r = rng(160)
  const end = Math.floor(now / HOUR)
  const start = end - 56 * 24 + 1
  const cells: CostCells = { hour: [], root: [], model: [], provider: [], kind: [], via: [], basis: [], nanos: [], calls: [], failed: [], repriced: [], input: [], output: [], cache_read: [], cache_write: [], cache_saved_nanos: [], approx_nanos: [] }
  const push = (h: number, root: number, model: number, kind: number, via: number, basis: number, calls: number, failed: number, input: number, output: number, read: number, write: number, approx = 0, repriced = 0) => {
    const m = MODELS[model]!
    const uncached = Math.max(0, input - read - write)
    const money = basis === 3 ? 0 : Math.round(((uncached * m[3] + read * m[5] + write * m[3] * 1.25 + output * m[4]) / 1e6) * 1e9)
    cells.hour.push(h)
    cells.root.push(root)
    cells.model.push(model)
    cells.provider.push(m[2])
    cells.kind.push(kind)
    cells.via.push(via)
    cells.basis.push(basis)
    cells.nanos.push(money)
    cells.calls.push(calls)
    cells.failed.push(failed)
    cells.repriced.push(repriced)
    cells.input.push(input)
    cells.output.push(output)
    cells.cache_read.push(read)
    cells.cache_write.push(write)
    cells.cache_saved_nanos.push(Math.round(((read * (m[3] - m[5])) / 1e6) * 1e9))
    cells.approx_nanos.push(approx === 1 ? money : 0)
  }
  const weights = ROOTS.map((x) => x.weight)
  const wsum = weights.reduce((a, b) => a + b, 0)
  const pick = () => {
    let x = r() * wsum
    for (let i = 0; i < weights.length; i++) if ((x -= weights[i]!) <= 0) return i
    return weights.length - 1
  }
  for (let h = start; h <= end; h++) {
    const lh = new Date(h * HOUR).getHours()
    const weekday = (new Date(h * HOUR).getDay() + 6) % 7
    const day = Math.floor((h - start) / 24)
    // Busy weeks and quiet weeks; days and evenings busier than nights; weekends lighter.
    const season = 0.35 + 0.65 * Math.abs(Math.sin(day / 9))
    const busy = (lh >= 8 && lh <= 23 ? 1 : 0.15) * (weekday >= 5 ? 0.55 : 1) * season
    const n = Math.floor(r() * 4 * busy + busy)
    for (let k = 0; k < n; k++) {
      const root = pick()
      const R = ROOTS[root]!
      if (R.deleted && day > 40) continue
      if (R.via) {
        // Reported leg: an external harness's money, partly placed by turn weight (approximate hour).
        const calls = 2 + Math.floor(r() * 10)
        push(h, root, 1, 6, R.via, 1, calls, 0, calls * 60_000, calls * 900, calls * 52_000, calls * 2_000, r() < 0.2 ? 1 : 0)
        continue
      }
      const model = R.owner === 'theuth' ? (r() < 0.75 ? 0 : 3) : R.owner_kind === 'routine' ? (r() < 0.6 ? 2 : 4) : r() < 0.55 ? 1 : r() < 0.7 ? 0 : r() < 0.85 ? 5 : r() < 0.95 ? 4 : 6
      const kind = R.owner === 'theuth' && r() < 0.45 ? 1 : R.owner_kind === 'routine' ? (R.owner === 'dreaming' ? 4 : 0) : r() < 0.08 ? 2 : r() < 0.05 ? 3 : r() < 0.04 ? 5 : 0
      const calls = kind === 1 ? 10 + Math.floor(r() * 40) : 1 + Math.floor(r() * 12)
      const per = kind === 1 ? 60_000 : 90_000 + Math.floor(r() * 60_000)
      const input = calls * per
      const read = Math.floor(input * (0.85 + r() * 0.12))
      const write = Math.floor((input - read) * 0.6)
      const output = calls * (kind === 1 ? 300 : 800 + Math.floor(r() * 1200))
      // Bedrock has no price here (unpriced, counted); a known-price slice; everything else estimated.
      const basis = model === 6 ? 3 : R.owner === 'ops-desk' && r() < 0.5 ? 0 : 2
      push(h, root, model, kind, 0, basis, calls, r() < 0.1 ? 1 : 0, basis === 3 ? 0 : input, basis === 3 ? 0 : output, basis === 3 ? 0 : read, basis === 3 ? 0 : write)
      // The endless compactor's failure loop: attempts with no usage, no money.
      if (kind === 1 && day > 14 && day < 22) push(h, root, 2, 1, 0, 3, 5 + Math.floor(r() * 20), 5, 0, 0, 0, 0)
    }
  }
  const roots: CostRootEntry[] = ROOTS.map((x, ix) => ({
    ix,
    root: { id: `sim-root-${String(ix).padStart(2, '0')}`, title: x.deleted ? '' : x.title, owner_kind: x.owner_kind, owner: x.owner, project: x.project, deleted: !!x.deleted, created_at_ms: (start + ix * 37) * HOUR },
  }))
  return {
    epoch: 'sim-epoch',
    rev: 1,
    full: true,
    hours: [],
    providers: PROVIDERS,
    kinds: KINDS,
    vias: VIAS,
    models: MODELS.map(([id, name, p]) => ({ id, name, provider: PROVIDERS[p]! })),
    roots,
    cells,
    meta: { engine: COSTS_FIXTURE_ENGINE, first_call_ms: start * HOUR, built_at_ms: now, pre_ledger_sessions: 86, unpriced: [{ reason: 'no usage returned', calls: 2100 }, { reason: 'no price for model', calls: 310 }, { reason: 'cancelled', calls: 120 }], price_table: 'sim' },
  }
}

/** Individual calls behind the fixture cube, newest first — up to four per cell, splitting its money and tokens. */
function fixtureCalls(cube: CostCube): CostCall[] {
  const c = cube.cells
  const out: CostCall[] = []
  for (let i = 0; i < c.hour.length; i++) {
    const n = Math.min(4, c.calls[i]!)
    const model = cube.models[c.model[i]!]!
    const basis = BASES[c.basis[i]!]!
    for (let k = 0; k < n; k++) {
      out.push({
        id: `sim-call-${i}-${k}`,
        at_ms: c.hour[i]! * HOUR + Math.floor(((k + 0.5) * HOUR) / n),
        root: cube.roots[c.root[i]!]!.root.id,
        session: cube.roots[c.root[i]!]!.root.id,
        via: cube.vias[c.via[i]!]!,
        provider: model.provider,
        model: model.id,
        kind: cube.kinds[c.kind[i]!]!,
        status: c.failed[i]! > k ? 'failed' : 'settled',
        basis,
        nanos: basis === 'unpriced' ? null : Math.round(c.nanos[i]! / n),
        input: c.input[i]! ? Math.round(c.input[i]! / n) : null,
        output: c.output[i]! ? Math.round(c.output[i]! / n) : null,
        cache_read: c.cache_read[i]! ? Math.round(c.cache_read[i]! / n) : null,
        cache_write: c.cache_write[i]! ? Math.round(c.cache_write[i]! / n) : null,
        repriced: false,
        reason: basis === 'unpriced' ? 'no usage returned' : null,
      })
    }
  }
  return out.sort((a, b) => b.at_ms - a.at_ms || (a.id < b.id ? 1 : -1))
}

let callsMemo: { cube: CostCube; calls: CostCall[] } | undefined

/** `cost_calls` over the fixture: the same filters and keyset paging as the engine. */
export function costCallsFixture(cube: CostCube, q: CostCallsQuery): CostCallsPage {
  if (callsMemo?.cube !== cube) callsMemo = { cube, calls: fixtureCalls(cube) }
  const limit = Math.min(1000, q.limit ?? 200)
  const ok = (v: string | null, list: string[] | null) => !list || (v !== null && list.includes(v))
  const page: CostCall[] = []
  for (const c of callsMemo.calls) {
    if (q.before && (c.at_ms > q.before.at_ms || (c.at_ms === q.before.at_ms && c.id >= q.before.id))) continue
    if ((q.from_ms !== null && c.at_ms < q.from_ms) || (q.to_ms !== null && c.at_ms >= q.to_ms)) continue
    if (!ok(c.root, q.roots) || !ok(c.provider, q.providers) || !ok(c.model, q.models) || !ok(c.kind, q.kinds) || !ok(c.via, q.vias) || !(q.bases ?? [c.basis]).includes(c.basis)) continue
    page.push(c)
    if (page.length === limit) break
  }
  const last = page.at(-1)
  return { calls: page, next: page.length === limit && last ? { at_ms: last.at_ms, id: last.id } : null }
}

/** A usage-limits snapshot with a provider row in every status and two budgets. */
export function limitsFixture(now: number): UsageLimits {
  const at = (min: number) => now + min * 60_000
  return {
    generated_at_ms: now,
    providers: [
      {
        id: 'anthropic:oauth', provider: 'anthropic', label: 'Claude subscription', auth: 'oauth', account: 'user@…', plan: 'max', status: 'warn', message: null, source: 'usage endpoint', observed_at_ms: at(-2),
        windows: [
          { id: 'five_hour', label: '5-hour', used_fraction: 0.62, resets_at_ms: at(140), window_secs: 18_000 },
          { id: 'seven_day', label: 'Weekly', used_fraction: 0.41, resets_at_ms: at(3 * 1440 + 200), window_secs: 604_800 },
          { id: 'seven_day_opus', label: 'Weekly · Opus', used_fraction: 0.78, resets_at_ms: at(3 * 1440 + 200), window_secs: 604_800 },
        ],
        rates: [], balance: null,
      },
      {
        id: 'anthropic:api_key', provider: 'anthropic', label: 'Anthropic API', auth: 'api_key', account: null, plan: 'tier 4', status: 'ok', message: null, source: 'response headers', observed_at_ms: now - 9_000,
        windows: [],
        rates: [
          { id: 'requests', label: 'Requests / min', scope: null, limit: 4000, remaining: 3960, resets_at_ms: now + 11_000 },
          { id: 'input_tokens', label: 'Input tokens / min', scope: 'claude-opus', limit: 2_000_000, remaining: 380_000, resets_at_ms: now + 11_000 },
          { id: 'output_tokens', label: 'Output tokens / min', scope: 'claude-opus', limit: 400_000, remaining: 388_000, resets_at_ms: now + 11_000 },
        ],
        balance: null,
      },
      {
        id: 'openai:oauth', provider: 'openai', label: 'ChatGPT · Codex', auth: 'oauth', account: 'acct-…4f2', plan: 'pro', status: 'limited', message: '5-hour window exhausted', source: 'headers + usage endpoint', observed_at_ms: at(-4),
        windows: [
          { id: 'primary', label: '5-hour', used_fraction: 1, resets_at_ms: at(38), window_secs: 18_000 },
          { id: 'secondary', label: 'Weekly', used_fraction: 0.33, resets_at_ms: at(5 * 1440), window_secs: 604_800 },
        ],
        rates: [], balance: { label: 'Credits', currency: 'credits', remaining: 0, limit: null, used: null },
      },
      {
        id: 'openrouter', provider: 'openrouter', label: 'OpenRouter', auth: 'api_key', account: null, plan: null, status: 'stale', message: 'last poll timed out; showing the last good values', source: 'usage endpoint', observed_at_ms: at(-48),
        windows: [], rates: [], balance: { label: 'Key limit', currency: 'USD', remaining: 31.4, limit: 50, used: 18.6 },
      },
      { id: 'opencode-go', provider: 'opencode', label: 'OpenCode Go', auth: 'login', account: null, plan: null, status: 'error', message: 'login expired — run /login opencode', source: 'usage endpoint', observed_at_ms: null, windows: [], rates: [], balance: null },
      { id: 'amazon-bedrock', provider: 'amazon-bedrock', label: 'Amazon Bedrock', auth: 'aws', account: 'profile sim', plan: null, status: 'unavailable', message: 'Bedrock exposes no usage meter; quotas live in AWS Service Quotas.', source: 'none', observed_at_ms: null, windows: [], rates: [], balance: null },
      { id: 'groq', provider: 'groq', label: 'Groq', auth: 'api_key', account: null, plan: null, status: 'not_configured', message: null, source: 'none', observed_at_ms: null, windows: [], rates: [], balance: null },
    ],
    budgets: [
      { id: 'agent:theuth', label: 'theuth desk', period: 'month', cap_usd: 2000, spent_usd: 1336.4, resets_at_ms: at(20 * 1440) },
      { id: 'session:sim-root-14', label: 'dreaming', period: 'night', cap_usd: 25, spent_usd: 23.1, resets_at_ms: at(600) },
    ],
  }
}
