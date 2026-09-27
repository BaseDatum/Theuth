// Settings › Search form state (16 §8): pure helpers over the `SearchSettings` wire type so the
// pane's reorder / edit / dirty logic is testable without rendering.

import type { ProviderSettings } from '@/rpc/bindings/ProviderSettings'
import type { SearchDefaults } from '@/rpc/bindings/SearchDefaults'
import type { SearchSettings } from '@/rpc/bindings/SearchSettings'

/** Human labels and one-line purpose per provider name. */
export const PROVIDER_INFO: Record<string, { label: string; blurb: string }> = {
  searxng: { label: 'SearXNG', blurb: 'Self-hosted metasearch; keyless. Needs the instance URL and its JSON format enabled.' },
  openserp: { label: 'OpenSERP', blurb: 'Self-hosted browser SERP for Google, Bing, DuckDuckGo and more; keyless locally, a key for the hosted API.' },
  firecrawl: { label: 'Firecrawl', blurb: 'Hosted or self-hosted search plus rendering; metered. Needs an API key.' },
  brave_api: { label: 'Brave Search API', blurb: 'Hosted API with news, FAQ and infobox features; metered. Needs a subscription token.' },
}

export function label(name: string): string {
  return PROVIDER_INFO[name]?.label ?? name
}

/** Providers sorted by priority (stable on ties by name). */
export function byPriority(list: readonly ProviderSettings[]): ProviderSettings[] {
  return [...list].sort((a, b) => a.priority - b.priority || a.name.localeCompare(b.name))
}

/** Replace one provider's fields by name. */
export function updateProvider(list: readonly ProviderSettings[], name: string, patch: Partial<ProviderSettings>): ProviderSettings[] {
  return list.map((p) => (p.name === name ? { ...p, ...patch } : p))
}

/**
 * Move `name` one step up (`-1`) or down (`+1`) in the priority order and renumber priorities
 * 10, 20, 30, … so the order is what is persisted. Ends are no-ops.
 */
export function moveProvider(list: readonly ProviderSettings[], name: string, dir: -1 | 1): ProviderSettings[] {
  const sorted = byPriority(list)
  const i = sorted.findIndex((p) => p.name === name)
  const j = i + dir
  if (i < 0 || j < 0 || j >= sorted.length) return sorted
  ;[sorted[i], sorted[j]] = [sorted[j] as ProviderSettings, sorted[i] as ProviderSettings]
  return sorted.map((p, k) => ({ ...p, priority: (k + 1) * 10 }))
}

/** Field-level validation mirroring the daemon's bounds; `null` when clean. */
export function validate(s: SearchSettings): string | null {
  for (const p of s.providers) {
    if (p.timeout_ms < 500 || p.timeout_ms > 60_000) return `${label(p.name)}: timeout must be 500..60000 ms`
    const url = (p.base_url ?? '').trim()
    if (url && !/^https?:\/\//.test(url)) return `${label(p.name)}: URL must start with http:// or https://`
    if (p.enabled && p.name === 'searxng' && !url) return 'SearXNG: enabled without an instance URL'
  }
  const d = s.defaults
  if (d.read_top < 0 || d.read_top > 8) return 'Read count must be 0..8'
  if (d.read_budget_ms < 500 || d.read_budget_ms > 120_000) return 'Read budget must be 0.5..120 s'
  if (d.render_cap_ms < 500 || d.render_cap_ms > 60_000) return 'Render cap must be 0.5..60 s'
  return null
}

/** Whether the edited form differs from what the daemon last returned. */
export function isDirty(saved: SearchSettings | undefined, draft: SearchSettings | undefined): boolean {
  if (!saved || !draft) return false
  return JSON.stringify(strip(saved)) !== JSON.stringify(strip(draft))
}

/** Drop the read-only fields (`secret_set`, `registered`) before comparing / sending. */
export function strip(s: SearchSettings): SearchSettings {
  return {
    defaults: s.defaults,
    providers: byPriority(s.providers).map((p) => ({ ...p, secret_set: false, registered: false })),
  }
}

/** Seconds typed in the form → milliseconds on the wire; junk keeps the previous value. */
export function msOf(seconds: string, fallback: number): number {
  const n = Number(seconds)
  return Number.isFinite(n) && n > 0 ? Math.round(n * 1000) : fallback
}

/** Human summary of one provider's registration state for the card footer. */
export function providerState(p: ProviderSettings): string {
  if (!p.enabled) return 'Disabled'
  if (p.secret && !p.secret_set) return `Key "${p.secret}" not set`
  if (p.name === 'searxng' && !(p.base_url ?? '').trim()) return 'No instance URL'
  return p.registered ? 'Registered' : 'Not registered yet (save to apply)'
}

export type { ProviderSettings, SearchDefaults, SearchSettings }
