// 177 U7: the local-server RPCs answered from memory, for screenshots of Connections › Local servers, a provider's
// model limits, the pickers' local tags and Settings › Memory's Test summary call. Dev only, behind
// `localStorage.local-fixture` (or `VITE_LOCAL_FIXTURE`): `"1"` — the test call succeeds; `"fail"` — it fails on thinking.
// Every other call is the engine's; an omlx provider `mlx` (Sky's setup) is added to its providers and models.

import type { Api } from '@/rpc/api'
import type { LocalServer } from '@/rpc/bindings/LocalServer'
import type { ModelInfo } from '@/rpc/bindings/ModelInfo'
import type { ProviderInfo } from '@/rpc/bindings/ProviderInfo'
import type { ProviderModel } from '@/rpc/bindings/ProviderModel'
import type { SummaryTest } from '@/rpc/bindings/SummaryTest'

const flag = (): string | null => (import.meta.env.DEV ? globalThis.localStorage?.getItem('local-fixture') || import.meta.env.VITE_LOCAL_FIXTURE || null : null)

const URL_MLX = 'http://127.0.0.1:8080/v1/'

const mlx: ProviderInfo = { name: 'mlx', base_url: URL_MLX, models: 3, builtin: false, configured: true, refreshable: true, local: true, runtime: 'omlx', has_key: true }

/** Ollama answers with 3 models, LM Studio is not running, omlx on :8080 is already `mlx`, nothing on :8000. */
const servers: LocalServer[] = [
  { runtime: 'ollama', base_url: 'http://127.0.0.1:11434/v1/', reachable: true, models: 3, added_as: null, error: null },
  { runtime: 'lmstudio', base_url: 'http://127.0.0.1:1234/v1/', reachable: false, models: 0, added_as: null, error: 'connection refused' },
  { runtime: 'omlx', base_url: 'http://127.0.0.1:8000/v1/', reachable: false, models: 0, added_as: null, error: 'connection refused' },
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

const later = <T>(v: T, ms: number) => new Promise<T>((resolve) => setTimeout(() => resolve(v), ms))

const wrapped = new WeakMap<Api, Api>()

/** The engine's `Api` with the 177 methods answered here when the fixture is on; the same object otherwise. One wrapper per `api`. */
export function localFixture(api: Api): Api {
  if (!flag()) return api
  const hit = wrapped.get(api)
  if (hit) return hit
  const fixture: Api = {
    ...api,
    list_providers: () => api.list_providers().then((rows) => [...rows.filter((p) => p.name !== 'mlx'), mlx]),
    list_models: () => api.list_models().then((rows) => [...rows.filter((m) => m.provider !== 'mlx'), ...limits.map(model)]),
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
  }
  wrapped.set(api, fixture)
  return fixture
}
