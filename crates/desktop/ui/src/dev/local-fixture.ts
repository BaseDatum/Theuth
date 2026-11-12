// 177 U7: the local-server RPCs answered from memory, for screenshots of Connections › Local servers, a provider's
// model limits, the pickers' local tags and Settings › Memory's Test summary call. Dev only, behind
// `localStorage.local-fixture` (or `VITE_LOCAL_FIXTURE`): `"1"` — the test call succeeds; `"fail"` — it fails on thinking.
// Every other call is the engine's; an omlx provider `mlx` (Sky's setup) is added to its providers and models.
// 191 D8: Network › Contacts answers from `contacts` below (a sound contact with a machine that left, an alert, a
// refused and a waiting record); `dev/contacts-fixture.html` renders them with no engine.
// 194: the new effort RPCs (`*_effort` / `set_*_effort`, `set_advisor_default_model`) answered from `efforts` below, so a
// chip pick round-trips (the setter stores, the getter returns it); `dev/effort-fixture.html` renders every picker.

import type { Api } from '@/rpc/api'
import type { AdvisorDefaults } from '@/rpc/bindings/AdvisorDefaults'
import type { LocalServer } from '@/rpc/bindings/LocalServer'
import type { ModelInfo } from '@/rpc/bindings/ModelInfo'
import type { ProviderInfo } from '@/rpc/bindings/ProviderInfo'
import type { Contact } from '@/rpc/bindings/Contact'
import type { Export } from '@/rpc/bindings/Export'
import type { ProviderModel } from '@/rpc/bindings/ProviderModel'
import type { SummaryTest } from '@/rpc/bindings/SummaryTest'

const flag = (): string | null => (import.meta.env.DEV ? globalThis.localStorage?.getItem('local-fixture') || import.meta.env.VITE_LOCAL_FIXTURE || null : null)

const URL_MLX = 'http://127.0.0.1:8080/v1/'

const mlx: ProviderInfo = { name: 'mlx', base_url: URL_MLX, models: 3, builtin: false, configured: true, refreshable: true, local: true, runtime: 'omlx', has_key: true }

/** Ollama answers with 3 models, LM Studio is not running, a second omlx on :8000 wants a key (401), omlx on :8080 is already `mlx`. */
const servers: LocalServer[] = [
  { runtime: 'ollama', base_url: 'http://127.0.0.1:11434/v1/', reachable: true, models: 3, added_as: null, error: null },
  { runtime: 'lmstudio', base_url: 'http://127.0.0.1:1234/v1/', reachable: false, models: 0, added_as: null, error: 'connection refused' },
  { runtime: 'omlx', base_url: 'http://127.0.0.1:8000/v1/', reachable: true, models: 0, added_as: null, error: 'HTTP 401 Unauthorized' },
  { runtime: 'omlx', base_url: URL_MLX, reachable: true, models: 3, added_as: 'mlx', error: null },
]

/** Mixed limit sources: unknown (assumed 16k), from the server, set by the user. */
let limits: ProviderModel[] = [
  { id: 'LFM3-1.7B-MLX-bf16', name: 'LFM3-1.7B-MLX-bf16', context: 0, output: 0, source: 'unknown', assumed_context: 16_384 },
  { id: 'Qwen3-4B-Instruct-2507-4bit', name: 'Qwen3-4B-Instruct-2507-4bit', context: 40_960, output: 0, source: 'server', assumed_context: null },
  { id: 'gemma-3-1b-it-qat-4bit', name: 'gemma-3-1b-it-qat-4bit', context: 32_768, output: 2_048, source: 'user', assumed_context: null },
]

const model = (m: ProviderModel): ModelInfo => ({ id: `mlx/${m.id}`, name: m.name, provider: 'mlx', context: m.context, reasoning: false, image: false, efforts: [], budget_min: null, budget_max: null, local: true })

const TEST_MODEL = 'mlx/LFM3-1.7B-MLX-bf16'
const ok: SummaryTest = {
  ok: true,
  model: TEST_MODEL,
  latency_ms: 2_140n,
  tokens_in: 1_873,
  tokens_out: 41,
  reasoning_tokens: null,
  finish_reason: 'stop',
  reasoning_seen: false,
  thinking_off_sent: true,
  line: 'Sky asks why omlx refuses max_tokens 0; the engine now omits max_tokens when the output limit is unknown.',
  error: null,
  hint: null,
}
const failed: SummaryTest = {
  ok: false,
  model: TEST_MODEL,
  latency_ms: 21_600n,
  tokens_in: 1_873,
  tokens_out: 2_048,
  reasoning_tokens: 2_048,
  finish_reason: 'length',
  reasoning_seen: true,
  thinking_off_sent: true,
  line: null,
  error: 'mlx/LFM3-1.7B-MLX-bf16 used its whole 2048-token reply on thinking (finish_reason=length)',
  hint: 'The server ignored enable_thinking: false. Turn thinking off for this model in omlx (model settings › enable_thinking), or pick a model without a thinking template.',
}

const NOW = Date.now()
const contact = (c: Partial<Contact> & Pick<Contact, 'name' | 'id'>): Contact => ({
  machine: 'laptop',
  endpoint: 'e7ac7242a74ef96fc15d964ef848e564f6971cf2bc08b08a94308c4a6e020825',
  fingerprint: '46WH EQVH J34W 7QK5 SZHP',
  path: 'offline',
  exports: [],
  first_contact: {},
  blocked: false,
  paired_ms: NOW - 86_400_000,
  shared: [],
  full_capable: true,
  problem_kind: null,
  paired_on: 'studio',
  machines: [],
  ...c,
})
const machine = (eid: string, name: string, left = false) => ({ eid, name, left })
/** One of each D8 row: sound (one of its machines left), alert, refused, waiting (a rename in flight, so two `bob`s). */
const contacts: Contact[] = [
  contact({
    name: 'bob',
    id: 'r-bob',
    machine: "Bob's MacBook",
    path: 'lan',
    exports: ['research'],
    first_contact: { research: 'allow' },
    machines: [machine('k3j9q2m4x7p1r8t5', "Bob's MacBook"), machine('p8t5w6y0k3j9q2m4', 'workshop'), machine('z9y8x7w6v5u4t3s2', 'old-imac', true)],
  }),
  contact({
    name: 'carol',
    id: 'r-carol',
    machine: 'thinkpad',
    path: 'relay',
    paired_on: 'xeus',
    problem: 'a machine of carol follows a roster we never signed; one of ours may be compromised',
    problem_kind: 'alert',
    machines: [machine('c4r0l5x7p1r8t5w6', 'thinkpad')],
  }),
  contact({ name: 'dave', id: 'r-dave', machine: 'pi', paired_on: '3f9a1c07', problem: "cannot verify this contact's machines", problem_kind: 'refused', machines: [machine('d4v3q2m4x7p1r8t5', 'd4v3q2m4')] }),
  contact({ name: 'bob', id: 'r-bob-2', machine: 'phone', problem: 'its name is taken by another contact (renaming)', problem_kind: 'waiting', machines: [machine('b0b2q2m4x7p1r8t5', 'b0b2q2m4')] }),
]
const exports: Export[] = [{ id: 'x-research', name: 'research', target: { kind: 'agent', agent: 'research@studio' }, description: 'research@studio', paused: null }]

const later = <T>(v: T, ms: number) => new Promise<T>((resolve) => setTimeout(() => resolve(v), ms))

/** Remove on `mlx` hides it (and its models) until reload. */
let removed = false

/** 194: the stored efforts by setting, and the advisor defaults once a setter replaced the engine's `Info.advisor`. */
const efforts: Record<'title' | 'polish' | 'vision' | 'escalate' | 'subagent' | 'theuth', string | null> = { title: null, polish: null, vision: null, escalate: null, subagent: null, theuth: null }
let advisor: AdvisorDefaults | null = null
const keep = <K extends keyof typeof efforts>(k: K) => (e: string | null) => Promise.resolve((efforts[k] = e))

const wrapped = new WeakMap<Api, Api>()

/** The engine's `Api` with the 177 methods answered here when the fixture is on; the same object otherwise. One wrapper per `api`. */
export function localFixture(api: Api): Api {
  if (!flag()) return api
  const hit = wrapped.get(api)
  if (hit) return hit
  const advisorNow = (): Promise<AdvisorDefaults> => fixture.info().then((i) => i.advisor ?? Promise.reject(new Error('engine sent no advisor defaults')))
  const fixture: Api = {
    ...api,
    list_providers: () => api.list_providers().then((rows) => [...rows.filter((p) => p.name !== 'mlx'), ...(removed ? [] : [mlx])]),
    list_models: () => api.list_models().then((rows) => [...rows.filter((m) => m.provider !== 'mlx'), ...(removed ? [] : limits.map(model))]),
    remove_provider: (name) => (name === 'mlx' ? ((removed = true), Promise.resolve(null)) : api.remove_provider(name)),
    local_servers_probe: () => later(servers, 600),
    provider_models: (provider) => (provider === 'mlx' ? Promise.resolve(limits) : api.provider_models(provider)),
    set_model_limits: (provider, id, context, output) => {
      if (provider !== 'mlx') return api.set_model_limits(provider, id, context, output)
      const m: ProviderModel = { ...limits.find((x) => x.id === id)!, context: context ?? 0, output: output ?? 0 }
      m.source = m.context || m.output ? 'user' : 'unknown'
      m.assumed_context = m.context ? null : 16_384
      limits = limits.map((x) => (x.id === id ? m : x))
      return Promise.resolve(m)
    },
    test_summary_call: (m) => later({ ...(flag() === 'fail' ? failed : ok), model: m }, 1_500),
    contact_list: () => Promise.resolve(contacts),
    contact_remove: () => Promise.resolve(null),
    contact_first_waiting: () => Promise.resolve([]),
    a2a_exports: () => Promise.resolve(exports),
    title_effort: () => Promise.resolve(efforts.title),
    set_title_effort: keep('title'),
    polish_effort: () => Promise.resolve(efforts.polish),
    set_polish_effort: keep('polish'),
    vision_effort: () => Promise.resolve(efforts.vision),
    set_vision_effort: keep('vision'),
    escalate_effort: () => Promise.resolve(efforts.escalate),
    set_escalate_effort: keep('escalate'),
    subagent_effort: () => Promise.resolve(efforts.subagent),
    set_subagent_effort: keep('subagent'),
    theuth_agent_effort: () => Promise.resolve(efforts.theuth),
    set_theuth_agent_effort: keep('theuth'),
    info: () => api.info().then((i) => ({ ...i, advisor: advisor ?? i.advisor })),
    set_advisor_default_model: (model) => advisorNow().then((a) => (advisor = { ...a, model, effort: model ? a.effort : null })),
    set_advisor_default_effort: (effort) => advisorNow().then((a) => (advisor = { ...a, effort })),
  }
  wrapped.set(api, fixture)
  return fixture
}
