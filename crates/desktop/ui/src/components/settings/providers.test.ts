import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { queryKeys } from '@/lib/rpc'
import type * as Rpc from '@/lib/rpc'
import type { LocalServer } from '@/rpc/bindings/LocalServer'
import type { ProviderInfo } from '@/rpc/bindings/ProviderInfo'
import type { ProviderModel } from '@/rpc/bindings/ProviderModel'
import { Connections, CustomProviderForm, LocalServerForm, ModelLimits, ProbeRows } from './Connections'
import {
  BUILTIN_LABEL,
  builtinKeyNote,
  contextPlaceholder,
  isLocalUrl,
  limitsSave,
  limitValue,
  localAddArgs,
  orderProviders,
  presetForm,
  probeForm,
  probeLine,
  refreshedText,
  RUNTIMES,
} from './providers'

vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => ({}) }))

const row = (name: string, builtin: boolean, configured: boolean, refreshable = false, local = false): ProviderInfo => ({
  name,
  base_url: local ? 'http://127.0.0.1:8080/v1/' : `https://${name}/v1/`,
  models: 1,
  builtin,
  configured,
  refreshable,
  local,
  runtime: local ? 'omlx' : null,
  has_key: configured,
})

const withCache = (el: ReactElement, fill: (qc: QueryClient) => void = () => {}) => {
  const qc = new QueryClient()
  fill(qc)
  return renderToStaticMarkup(createElement(QueryClientProvider, { client: qc }, el))
}
const render = (providers: ProviderInfo[]) => withCache(createElement(Connections), (qc) => qc.setQueryData(queryKeys.providers, providers))
const text = (html: string) => html.replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ')

test('built-ins come first, custom providers after, engine order kept within each', () => {
  const list = orderProviders([row('a', false, true), row('opencode', true, false), row('rwx', false, true), row('openrouter', true, true)])
  expect(list.map((p) => p.name)).toEqual(['opencode', 'openrouter', 'a', 'rwx'])
  expect(orderProviders(undefined)).toEqual([])
})

test('refresh toast: counts on a change, "no change" when nothing moved', () => {
  const provider = { ...row('rwx', false, true, true), models: 14 }
  expect(refreshedText({ provider, added: ['m1', 'm2'], removed: ['m0'] })).toBe('rwx: 14 models (+2, −1)')
  expect(refreshedText({ provider, added: [], removed: ['m0'] })).toBe('rwx: 14 models (+0, −1)')
  expect(refreshedText({ provider, added: [], removed: [] })).toBe('rwx: 14 models, no change')
})

test('one Providers section: rows in order with tags, Refresh models only where refreshable, no "gateway"', () => {
  const html = render([row('opencode', true, true, true), row('openrouter', true, false, false), row('rwx', false, true, true)])
  const t = text(html)
  expect(t).toContain('Providers')
  expect(t.indexOf('OpenCode Zen')).toBeLessThan(t.indexOf('OpenRouter'))
  expect(t.indexOf('OpenRouter')).toBeLessThan(t.indexOf('rwx'))
  expect(t.match(/built-in/g)).toHaveLength(2)
  expect(t).toContain('custom')
  expect(t).toContain('https://rwx/v1/')
  expect(t).toContain('key set')
  expect(t).toContain('no key (secret `openrouter`)')
  // opencode and rwx are refreshable; openrouter (no key) is not.
  expect(t.match(/Refresh models/g)).toHaveLength(2)
  expect(html).toContain('aria-label="Remove rwx"')
  expect(html).not.toContain('aria-label="Remove opencode"')
  expect(html).toContain('aria-label="Custom provider name"')
  expect(t).not.toContain('No custom providers yet.')
  expect(html).not.toMatch(/gateway/i)
})

test('no custom provider: the muted empty line', () => {
  const html = render([row('opencode', true, false), row('openrouter', true, false)])
  expect(html).toContain('No custom providers yet.')
  expect(html).not.toContain('Refresh models')
  expect(html).not.toMatch(/gateway/i)
})

test('built-in labels and key notes', () => {
  expect(BUILTIN_LABEL.opencode).toBe('OpenCode Zen')
  expect(builtinKeyNote('openrouter')).toBe('OpenRouter key (Settings > Connections)')
  expect(builtinKeyNote('zen')).toBe('zen key (Settings > Connections)')
})

test('177 U1: Local servers sits between the built-ins and the custom providers; a local row has the "local" tag, address, count, Refresh models and Remove', () => {
  const html = render([row('opencode', true, true, true), row('rwx', false, true, true), row('mlx', false, true, true, true)])
  const t = text(html)
  expect(t.indexOf('OpenCode Zen')).toBeLessThan(t.indexOf('Local servers'))
  expect(t.indexOf('Local servers')).toBeLessThan(t.indexOf(' mlx '))
  expect(t.indexOf(' mlx ')).toBeLessThan(t.indexOf('Custom providers'))
  expect(t.indexOf('Custom providers')).toBeLessThan(t.indexOf(' rwx '))
  expect(t).toContain('Find running servers')
  expect(t).toMatch(/mlx local omlx · mlx\/… · 1 models · http:\/\/127\.0\.0\.1:8080\/v1\//)
  expect(t.match(/Refresh models/g)).toHaveLength(3)
  expect(html).toContain('aria-label="Remove mlx"')
  // Custom and local rows expand to their model limits; built-ins do not.
  expect(html).toContain('aria-label="Model limits of mlx"')
  expect(html).toContain('aria-label="Model limits of rwx"')
  expect(html).not.toContain('aria-label="Model limits of opencode"')
  expect(t).not.toContain('No local servers yet.')
  expect(text(render([row('opencode', true, true)]))).toContain('No local servers yet.')
})

const probe: LocalServer[] = [
  { runtime: 'ollama', base_url: 'http://127.0.0.1:11434/v1/', reachable: true, models: 3, added_as: null, error: null },
  { runtime: 'lmstudio', base_url: 'http://127.0.0.1:1234/v1/', reachable: false, models: 0, added_as: null, error: 'connection refused' },
  { runtime: 'omlx', base_url: 'http://127.0.0.1:8080/v1/', reachable: true, models: 2, added_as: 'mlx', error: null },
]

test('177 U1: probe rows — runtime, address, N models or not running, Add unless already added as a provider', () => {
  const html = renderToStaticMarkup(createElement(ProbeRows, { servers: probe, onAdd() {} }))
  const t = text(html)
  expect(t).toContain('Ollama http://127.0.0.1:11434/v1/ · 3 models')
  expect(t).toContain('LM Studio http://127.0.0.1:1234/v1/ · not running')
  expect(t).toContain('omlx http://127.0.0.1:8080/v1/ · 2 models')
  expect(t).toContain('Added as mlx')
  expect(html).toContain('aria-label="Add Ollama at http://127.0.0.1:11434/v1/"')
  expect(html).toContain('aria-label="Add LM Studio at http://127.0.0.1:1234/v1/"')
  expect(html).not.toContain('aria-label="Add omlx')
  expect(html).toContain('title="connection refused"')
  expect(text(renderToStaticMarkup(createElement(ProbeRows, { servers: [], onAdd() {} })))).toContain('Nothing answered on 127.0.0.1.')
  expect(probe.map(probeLine)).toEqual(['Ollama http://127.0.0.1:11434/v1/ · 3 models', 'LM Studio http://127.0.0.1:1234/v1/ · not running', 'omlx http://127.0.0.1:8080/v1/ · 2 models · added as mlx'])
})

test('177 U1: Add prefills the form with the runtime, its name and the address the server answered on', () => {
  expect(probeForm(probe[0]!)).toEqual({ runtime: 'ollama', name: 'ollama', url: 'http://127.0.0.1:11434/v1/', key: '' })
  // An omlx on a non-default port keeps the probed address, not the preset's :8000.
  expect(probeForm({ ...probe[2]!, added_as: null })).toEqual({ runtime: 'omlx', name: 'omlx', url: 'http://127.0.0.1:8080/v1/', key: '' })
  const html = renderToStaticMarkup(createElement(LocalServerForm, { form: probeForm(probe[0]!), onChange() {}, onSubmit() {}, pending: false }))
  expect(html).toMatch(/<button[^>]*aria-label="Local server runtime"[^>]*>.*Ollama/)
  expect(html).toMatch(/value="ollama"[^>]*aria-label="Local server name"|aria-label="Local server name"[^>]*value="ollama"/)
  expect(html).toContain('value="http://127.0.0.1:11434/v1/"')
})

test('177 U1: the runtime presets fill name and address; the add call carries the runtime and a null key when none is typed', () => {
  expect(RUNTIMES.map((r) => r.label)).toEqual(['Ollama', 'LM Studio', 'omlx', 'llama.cpp', 'vLLM', 'Other OpenAI-compatible'])
  expect(presetForm('ollama')).toEqual({ runtime: 'ollama', name: 'ollama', url: 'http://127.0.0.1:11434/v1/', key: '' })
  expect(presetForm('lmstudio').url).toBe('http://127.0.0.1:1234/v1/')
  expect(presetForm('omlx').url).toBe('http://127.0.0.1:8000/v1/')
  expect(presetForm('llamacpp')).toMatchObject({ name: 'llamacpp', url: 'http://127.0.0.1:8080/v1/' })
  expect(presetForm('vllm')).toMatchObject({ name: 'vllm', url: 'http://127.0.0.1:8000/v1/' })
  expect(presetForm('openai')).toMatchObject({ name: 'local', url: '' })
  expect(localAddArgs({ ...presetForm('omlx'), name: ' mlx ' })).toEqual(['mlx', 'http://127.0.0.1:8000/v1/', null, 'omlx'])
  expect(localAddArgs({ ...presetForm('omlx'), key: ' sk-fake ' })).toEqual(['omlx', 'http://127.0.0.1:8000/v1/', 'sk-fake', 'omlx'])
  const html = renderToStaticMarkup(createElement(LocalServerForm, { form: presetForm('lmstudio'), onChange() {}, onSubmit() {}, pending: false }))
  expect(html).toContain('Only if your server asks for one (omlx with key verification on does)')
  expect(html).toContain('placeholder="API key (optional)"')
  expect(html).not.toMatch(/<button[^>]*type="submit"[^>]*disabled=""/)
  const empty = renderToStaticMarkup(createElement(LocalServerForm, { form: presetForm('openai'), onChange() {}, onSubmit() {}, pending: false }))
  expect(empty).toMatch(/<button[^>]*type="submit"[^>]*disabled=""/)
})

const limits: ProviderModel[] = [
  { id: 'LFM3-1.7B-MLX-bf16', name: 'LFM3-1.7B-MLX-bf16', context: 0, output: 0, source: 'unknown', assumed_context: 16_384 },
  { id: 'qwen3-4b', name: 'Qwen3 4B', context: 40_960, output: 0, source: 'server', assumed_context: null },
  { id: 'gemma-3-1b', name: 'Gemma 3 1B', context: 32_768, output: 2_048, source: 'user', assumed_context: null },
]

test('177 U2: the limits editor — Context/Output per model, "unknown" placeholders (assuming 16k for a local unknown), source tags', () => {
  const html = withCache(createElement(ModelLimits, { provider: 'mlx' }), (qc) => qc.setQueryData(queryKeys.providerModels('mlx'), limits))
  const field = (label: string) => html.match(new RegExp(`<input[^>]*aria-label="${label}"[^>]*>`))?.[0] ?? ''
  expect(field('Context of LFM3-1.7B-MLX-bf16')).toContain('placeholder="unknown · assuming 16k"')
  expect(field('Context of LFM3-1.7B-MLX-bf16')).toContain('value=""')
  expect(field('Output of LFM3-1.7B-MLX-bf16')).toContain('placeholder="unknown"')
  expect(field('Context of qwen3-4b')).toContain('value="40960"')
  expect(field('Context of qwen3-4b')).toContain('placeholder="unknown"')
  expect(field('Output of gemma-3-1b')).toContain('value="2048"')
  const t = text(html)
  expect(t).toMatch(/LFM3-1.7B-MLX-bf16 unknown .*Qwen3 4B from server .*Gemma 3 1B set by you/)
  expect(t.match(/Save/g)).toHaveLength(3)
  expect(t.match(/Clear/g)).toHaveLength(3)
  expect(contextPlaceholder({ ...limits[0]!, assumed_context: null })).toBe('unknown')
})

test('177 U2: Save sends the typed fields (an emptied one as null, i.e. unknown), nothing while unchanged or invalid', () => {
  const [unknown, server, user] = limits as [ProviderModel, ProviderModel, ProviderModel]
  expect(limitsSave(unknown, '32768', '')).toEqual({ id: 'LFM3-1.7B-MLX-bf16', context: 32_768, output: null })
  expect(limitsSave(unknown, ' 32768 ', '4096')).toEqual({ id: 'LFM3-1.7B-MLX-bf16', context: 32_768, output: 4_096 })
  expect(limitsSave(user, '32768', '')).toEqual({ id: 'gemma-3-1b', context: 32_768, output: null })
  expect(limitsSave(user, '', '')).toEqual({ id: 'gemma-3-1b', context: null, output: null })
  expect(limitsSave(unknown, '', '')).toBeNull()
  expect(limitsSave(server, '40960', '')).toBeNull()
  for (const bad of ['0', '-1', '1.5', 'abc', '1e12']) expect(limitsSave(unknown, bad, '')).toBeNull()
  expect([limitValue(''), limitValue('8192'), limitValue('0')]).toEqual([null, 8_192, undefined])
})

test('177 U3: the engine’s local-host rule — loopback, private, CGNAT, link-local, ULA, mapped v4, single-label and the local suffixes', () => {
  const local = [
    'http://127.0.0.1:8080/v1/',
    'http://localhost:11434/v1/',
    'http://10.0.0.5/v1/',
    'http://172.16.0.1/v1/',
    'http://172.31.255.1/v1/',
    'http://192.168.1.20:1234/v1/',
    'http://100.64.0.1/v1/',
    'http://100.127.255.255/v1/',
    'http://169.254.1.1/v1/',
    'http://0.0.0.0:8000/v1/',
    'http://[::1]:8080/v1/',
    'http://[fd12:3456::1]/v1/',
    'http://[fe80::1]/v1/',
    'http://[::ffff:192.168.1.2]/v1/',
    'http://mac-studio:8080/v1/',
    'http://studio.local/v1/',
    'http://box.lan/v1/',
    'http://gpu.internal/v1/',
    'http://nas.home.arpa/v1/',
    'http://mac.tail1234.ts.net/v1/',
    'http://app.localhost/v1/',
  ]
  const cloud = ['https://api.openai.com/v1/', 'http://172.32.0.1/v1/', 'http://100.128.0.1/v1/', 'http://8.8.8.8/v1/', 'http://[2001:db8::1]/v1/', 'http://[::ffff:8.8.8.8]/v1/', 'https://local.example.com/v1/', 'localhost:8080', '', 'not a url']
  expect(local.filter((u) => !isLocalUrl(u))).toEqual([])
  expect(cloud.filter((u) => isLocalUrl(u))).toEqual([])
})

test('177 U3: the custom form makes the key optional, and says why, only for a local address', () => {
  const html = (url: string, key = '') => renderToStaticMarkup(createElement(CustomProviderForm, { form: { name: 'box', url, key }, onChange() {}, onSubmit() {}, pending: false }))
  const keyField = (h: string) => h.match(/<input[^>]*aria-label="Custom provider API key"[^>]*>/)?.[0] ?? ''
  const submit = (h: string) => h.match(/<button[^>]*type="submit"[^>]*>/)?.[0] ?? ''
  const local = html('http://192.168.1.20:8080/v1/')
  expect(keyField(local)).toContain('placeholder="API key (optional)"')
  expect(keyField(local)).not.toContain('required')
  expect(submit(local)).not.toContain('disabled=""')
  expect(local).toContain('Key optional: this address is on your machine or network')
  const cloud = html('https://llm.example.com/v1/')
  expect(keyField(cloud)).toContain('placeholder="API key"')
  expect(keyField(cloud)).toContain('required')
  expect(submit(cloud)).toContain('disabled=""')
  expect(cloud).not.toContain('Key optional')
  expect(submit(html('https://llm.example.com/v1/', 'sk-fake'))).not.toContain('disabled=""')
})
