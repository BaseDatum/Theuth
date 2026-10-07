// 124 §2: word search over the model catalog — one spec, implemented here and in `crates/tui/src/model.rs`.
// No fuzzy matching: a term scores by where it sits (whole name / id tail, name start, word boundary,
// provider, plain substring), per-term plain substrings are gated out when any model matches at a boundary.

import type { ModelInfo } from '@/rpc/bindings/ModelInfo'

/** `query` lowercased and split on whitespace; empty → browse mode (§3), no scoring. */
export function searchTerms(query: string): string[] {
  return query.trim().toLowerCase().split(/\s+/).filter(Boolean)
}

/** Index of `t` in `s` where it starts a word (index 0 or preceded by a non-`[a-z0-9]`), else -1. */
function boundaryIndex(s: string, t: string): number {
  for (let i = s.indexOf(t); i >= 0; i = s.indexOf(t, i + 1)) {
    if (i === 0 || !/[a-z0-9]/.test(s[i - 1]!)) return i
  }
  return -1
}

/** §2.2 per-term tier of `m` for the lowercase term `t`: 100/90/80/70/65/60/40/30, 0 when nothing matches. */
export function termTier(m: ModelInfo, t: string): number {
  const name = m.name.toLowerCase()
  const id = m.id.toLowerCase()
  const provider = m.provider.toLowerCase()
  const slash = id.indexOf('/')
  const tail = slash < 0 ? id : id.slice(slash + 1)
  if (name === t || tail === t) return 100
  if (name.startsWith(t)) return 90
  if (boundaryIndex(name, t) > 0) return 80
  if (provider === t) return 70
  if (provider.startsWith(t)) return 65
  if (boundaryIndex(tail, t) >= 0) return 60
  if (name.includes(t)) return 40
  if (id.includes(t)) return 30
  return 0
}

/**
 * §2.2: models matching every term of `query`, score descending, ties in catalog order (`models`' order).
 * Per term, tiers 40/30 count only when no model reaches ≥ 60 for it. No terms → `models` unchanged.
 */
export function rankModels(models: ModelInfo[], query: string): ModelInfo[] {
  const terms = searchTerms(query)
  if (terms.length === 0) return models
  const scores = models.map(() => 0)
  for (const t of terms) {
    const tiers = models.map((m) => termTier(m, t))
    const gated = tiers.some((x) => x >= 60)
    tiers.forEach((x, i) => {
      if (scores[i]! < 0) return
      const tier = gated && x < 60 ? 0 : x
      scores[i] = tier === 0 ? -1 : scores[i]! + tier
    })
  }
  return models
    .map((m, i) => ({ m, score: scores[i]!, i }))
    .filter((x) => x.score > 0)
    .sort((a, b) => b.score - a.score || a.i - b.i)
    .map((x) => x.m)
}

/** Distinct providers of `models` with their model counts, A→Z by id. */
export function providersOf(models: ModelInfo[]): { id: string; count: number }[] {
  const counts = new Map<string, number>()
  for (const m of models) counts.set(m.provider, (counts.get(m.provider) ?? 0) + 1)
  return [...counts]
    .map(([id, count]) => ({ id, count }))
    .sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0))
}

/** The models of `provider`, sorted by name (case-insensitive), then id. */
export function modelsOfProvider(models: ModelInfo[], provider: string): ModelInfo[] {
  return models
    .filter((m) => m.provider === provider)
    .sort((a, b) => {
      const an = a.name.toLowerCase()
      const bn = b.name.toLowerCase()
      return an < bn ? -1 : an > bn ? 1 : a.id < b.id ? -1 : a.id > b.id ? 1 : 0
    })
}
