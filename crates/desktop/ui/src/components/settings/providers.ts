import type { LimitSource } from "@/rpc/bindings/method.agent.output"
import type { LocalRuntime } from "@/rpc/bindings/method.agent.output"
import type { LocalServer } from "@/rpc/bindings/method.agent.output"
import type { ProviderInfo } from "@/rpc/bindings/method.agent.output"
import type { ProviderModel } from "@/rpc/bindings/method.agent.output"
import type { RefreshedModels } from "@/rpc/bindings/method.agent.output"
import type { SummaryTest } from "@/rpc/bindings/method.agent.output"
import type { LoginInfo } from "@/rpc/bindings/method.agent.output"

/** Display names of the built-in key providers (85 §0.9); the engine sends the ids. */
export const BUILTIN_LABEL: Record<string, string> = {
  opencode: 'OpenCode Zen',
  openrouter: 'OpenRouter',
}

/** The one Models › Providers list (162 D2): built-ins first, then custom providers, engine order kept within each. */
export function orderProviders(list: ProviderInfo[] | undefined): ProviderInfo[] {
  return [...(list?.filter((p) => p.builtin) ?? []), ...(list?.filter((p) => !p.builtin) ?? [])]
}

/** Secret note written beside a built-in key, as the other Settings key forms do. */
export function builtinKeyNote(name: string) {
  return `${BUILTIN_LABEL[name] ?? name} key (Settings > Models > Providers)`
}

/**
 * 188 A6: the `list_logins` rows beside the provider rows. A login sits on the provider row of the same name
 * (`byProvider`); one that matches no row (`alone`) gets a row of its own. Engine order is kept in both.
 */
export function pairLogins(providers: readonly ProviderInfo[], logins: readonly LoginInfo[]): { byProvider: ReadonlyMap<string, LoginInfo>; alone: LoginInfo[] } {
  const names = new Set(providers.map((p) => p.name))
  return {
    byProvider: new Map(logins.filter((l) => names.has(l.name)).map((l) => [l.name, l])),
    alone: logins.filter((l) => !names.has(l.name)),
  }
}

/** A login's state: `not logged in`, `logged in`, or `logged in (<account>)` when the provider names it. */
export const loginText = (l: LoginInfo): string => (!l.logged_in ? 'not logged in' : l.account ? `logged in (${l.account})` : 'logged in')

/** Toast of a `refresh_provider_models` reply (162 D7): `rwx: 14 models (+2, −1)` or `rwx: 14 models, no change`. */
export function refreshedText({ provider: p, added, removed }: RefreshedModels): string {
  if (added.length === 0 && removed.length === 0) return `${p.name}: ${p.models} models, no change`
  return `${p.name}: ${p.models} models (+${added.length}, −${removed.length})`
}

/** 177 U1: the Add local server presets — label, the provider name it suggests and the default address (`openai` = any other OpenAI-compatible server, no default). */
export const RUNTIMES: readonly { runtime: LocalRuntime; label: string; name: string; url: string }[] = [
  { runtime: 'ollama', label: 'Ollama', name: 'ollama', url: 'http://127.0.0.1:11434/v1/' },
  { runtime: 'lmstudio', label: 'LM Studio', name: 'lmstudio', url: 'http://127.0.0.1:1234/v1/' },
  { runtime: 'omlx', label: 'omlx', name: 'omlx', url: 'http://127.0.0.1:8000/v1/' },
  { runtime: 'llamacpp', label: 'llama.cpp', name: 'llamacpp', url: 'http://127.0.0.1:8080/v1/' },
  { runtime: 'vllm', label: 'vLLM', name: 'vllm', url: 'http://127.0.0.1:8000/v1/' },
  { runtime: 'openai', label: 'Other OpenAI-compatible', name: 'local', url: '' },
]

const preset = (runtime: LocalRuntime) => RUNTIMES.find((r) => r.runtime === runtime) ?? RUNTIMES[RUNTIMES.length - 1]!

export const runtimeLabel = (runtime: LocalRuntime): string => preset(runtime).label

/** The Add local server form. */
export type LocalForm = { runtime: LocalRuntime; name: string; url: string; key: string }

/** Picking a runtime fills the name and the default address; both stay editable. */
export const presetForm = (runtime: LocalRuntime): LocalForm => ({ runtime, name: preset(runtime).name, url: preset(runtime).url, key: '' })

/** A probe row's Add: its runtime's name, the address it answered on. */
export const probeForm = (s: LocalServer): LocalForm => ({ ...presetForm(s.runtime), url: s.base_url })

/** `add_provider` arguments of the local form: an empty key is `null` (keyless), the runtime always sent. */
export const localAddArgs = (f: LocalForm): [string, string, string | null, LocalRuntime] => [f.name.trim(), f.url.trim(), f.key.trim() || null, f.runtime]

/** `name`, else the first free `name-2`, `name-3`, … — a prefilled name never replaces an existing provider (177 R1-3). */
export function freeName(name: string, taken: readonly string[]): string {
  if (!taken.includes(name)) return name
  let n = 2
  while (taken.includes(`${name}-${n}`)) n++
  return `${name}-${n}`
}

/**
 * 177 E4, the engine's `local::private_url` mirrored: a URL whose host is loopback, private (RFC 1918, CGNAT,
 * link-local, ULA, unspecified, v4-mapped), single-label, or under `.localhost` `.local` `.internal` `.lan` `.home.arpa` `.ts.net`.
 */
export function isLocalUrl(raw: string): boolean {
  let host: string
  try {
    host = new URL(raw.trim()).hostname.toLowerCase()
  } catch {
    return false
  }
  if (!host) return false
  if (host.startsWith('[')) return localV6(host.slice(1, -1))
  const v4 = host.match(/^(\d+)\.(\d+)\.(\d+)\.(\d+)$/)
  if (v4) return localV4(v4.slice(1).map(Number) as [number, number, number, number])
  return !host.includes('.') || ['.localhost', '.local', '.internal', '.lan', '.home.arpa', '.ts.net'].some((s) => host.endsWith(s))
}

function localV4([a, b, c, d]: [number, number, number, number]): boolean {
  return a === 127 || a === 10 || (a === 172 && b >= 16 && b <= 31) || (a === 192 && b === 168) || (a === 169 && b === 254) || (a | b | c | d) === 0 || (a === 100 && (b & 0xc0) === 64)
}

/** `URL` serializes IPv6 as compressed lowercase hex (no dotted tail), so `::` expansion is all it takes. */
function localV6(s: string): boolean {
  const part = (x: string | undefined) => (x ? x.split(':').map((h) => parseInt(h, 16)) : [])
  const [head, tail] = s.split('::')
  const h = part(head)
  const t = part(tail)
  const w = s.includes('::') ? [...h, ...Array<number>(8 - h.length - t.length).fill(0), ...t] : h
  const zeros = (n: number) => w.slice(0, n).every((x) => x === 0)
  if (zeros(7) && (w[7] === 0 || w[7] === 1)) return true
  if ((w[0]! & 0xfe00) === 0xfc00 || (w[0]! & 0xffc0) === 0xfe80) return true
  return zeros(5) && w[5] === 0xffff && localV4([w[6]! >> 8, w[6]! & 255, w[7]! >> 8, w[7]! & 255])
}

/** 177 U2: the source tag of a model's limits. */
export const LIMIT_SOURCE: Record<LimitSource, string> = { server: 'from server', user: 'set by you', unknown: 'unknown' }

/** The Context field's placeholder: "unknown", or what a local model is budgeted as meanwhile (177 E7). */
export const contextPlaceholder = (m: ProviderModel): string => (m.assumed_context ? `unknown · assuming ${Math.round(m.assumed_context / 1024)}k` : 'unknown')

/** One limit field: empty → `null` (unknown), a whole number ≥ 1 → itself, anything else → `undefined` (not savable). */
export function limitValue(raw: string): number | null | undefined {
  const s = raw.trim()
  if (s === '') return null
  const n = Number(s)
  return Number.isInteger(n) && n >= 1 && n <= 0xffff_ffff ? n : undefined
}

/** A `set_model_limits` call: the model and both fields, `null` = unknown. */
export type Limits = { id: string; context: number | null; output: number | null }

/** What Save sends for the typed fields (empty = `null`); `null` while a field is invalid or nothing changed. Clear sends `null` for both. */
export function limitsSave(m: ProviderModel, ctx: string, out: string): Limits | null {
  const context = limitValue(ctx)
  const output = limitValue(out)
  if (context === undefined || output === undefined) return null
  return (context ?? 0) !== m.context || (output ?? 0) !== m.output ? { id: m.id, context, output } : null
}

/** `set_model_limits` arguments in contract order: provider, model, context, output (`null` = unknown; Clear sends both `null`). */
export const limitsArgs = (provider: string, l: Limits): [string, string, number | null, number | null] => [provider, l.id, l.context, l.output]

/** `2.1 s · 1,873 in · 41 out · finish stop · thinking off sent` — the Test summary call's numbers, the reported ones only. */
export function summaryTestStats(t: SummaryTest): string {
  const n = (v: number) => v.toLocaleString('en-US')
  return [
    `${(Number(t.latency_ms) / 1000).toFixed(1)} s`,
    t.tokens_in !== null && `${n(t.tokens_in)} in`,
    t.tokens_out !== null && `${n(t.tokens_out)} out`,
    t.reasoning_tokens !== null && `${n(t.reasoning_tokens)} thinking`,
    t.finish_reason && `finish ${t.finish_reason}`,
    t.thinking_off_sent && 'thinking off sent',
  ]
    .filter(Boolean)
    .join(' · ')
}

/** A probe row after its address: the model count or "not running", the engine's error, and "may need a key" when a server answers with an error and no models (omlx with key verification on) — unless the error already says so. */
export function probeState(s: LocalServer): string {
  const hint = s.reachable && s.error && s.models === 0 && !/key/i.test(s.error) && 'may need a key'
  return [s.reachable ? `${s.models} models` : 'not running', s.error, hint].filter(Boolean).join(' · ')
}

/** One probe row as text (the slash `/provider find` toast). */
export function probeLine(s: LocalServer): string {
  return `${runtimeLabel(s.runtime)} ${s.base_url} · ${probeState(s)}${s.added_as ? ` · added as ${s.added_as}` : ''}`
}
