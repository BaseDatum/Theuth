// 224: the cloud-node UI's pure helpers — prices, names, statuses, the create job's stage rows, the destroy saving.
// No React, no RPC: the wizard, Settings › Machines and the tests share them.

import { type CloudNode, DEFAULT_VOLUME_GB, type K8sOptions, type NodeSpec, type ProviderInfo, type ProviderKind, type Quote, STAGES, type Stage } from './types'

/** The provider badges of the mockup: the brands' own colours (the only hard-coded colours of the cloud UI). */
export const BRAND: Record<ProviderKind, { bg: string; mark: string }> = {
  digitalocean: { bg: '#0069ff', mark: 'DO' },
  fly: { bg: '#7b3fe4', mark: 'F' },
  lightsail: { bg: '#ff9900', mark: 'A' },
  kubernetes: { bg: '#326ce5', mark: 'K' },
}

const cents = (n: number) => Math.round(n * 100) / 100

/** `$12`, `$12.19`: whole dollars bare, else cents. */
export function usd(n: number): string {
  const c = cents(n)
  return Number.isInteger(c) ? `$${c}` : `$${c.toFixed(2)}`
}

/** A list price for a card: `$12`, or `≈$11` for Fly's computed `$10.69`. */
export function approxUsd(n: number): string {
  const c = cents(n)
  return Number.isInteger(c) ? `$${c}` : `≈$${Math.round(c)}`
}

/** The server price of the 2 GB minimum; `null` when the provider names none (Kubernetes). */
export function minPrice(p: ProviderInfo): number | null {
  const sizes = p.sizes.filter((s) => s.monthly_usd !== null).sort((a, b) => a.memory_gb - b.memory_gb)
  return sizes[0]?.monthly_usd ?? null
}

/** The providers a screen offers: Kubernetes (`desktop_only`) never on a phone. */
export function offered(providers: readonly ProviderInfo[], phone: boolean): ProviderInfo[] {
  return providers.filter((p) => !(phone && p.desktop_only))
}

/**
 * The first-run chip: the cheapest 2 GB server across the providers that can create nodes here, beta ones (Fly.io)
 * aside so the headline price is one we stand behind (`from $12/mo`); beta prices only when nothing else is offered.
 */
export function fromPrice(providers: readonly ProviderInfo[], phone: boolean): string | null {
  const available = offered(providers, phone).filter((p) => p.available)
  const stable = available.filter((p) => !p.beta)
  const prices = (stable.length ? stable : available)
    .map(minPrice)
    .filter((n): n is number => n !== null)
  if (!prices.length) return null
  return `from $${Math.round(Math.min(...prices))}/mo`
}

/** The quote the wizard shows before `cloud_quote` answers (same arithmetic as `catalog::quote`). */
export function localQuote(p: ProviderInfo, memory_gb: number, volume_gb = DEFAULT_VOLUME_GB): Quote | null {
  const size = p.sizes.find((s) => s.memory_gb === memory_gb)
  if (!size) return null
  const disk = p.volume_usd_per_gb === null ? null : cents(p.volume_usd_per_gb * volume_gb)
  const total = size.monthly_usd !== null && disk !== null ? cents(size.monthly_usd + disk) : null
  return { provider: p.provider, size_slug: size.slug, server_usd: size.monthly_usd, disk_usd: disk, total_usd: total }
}

/** The create button: the press is the approval, so it names the monthly total (`Create node · $13/mo`). */
export function createLabel(spec: Pick<NodeSpec, 'provider' | 'k8s'>, quote: Quote | null): string {
  if (spec.provider === 'kubernetes') return `Deploy to ${spec.k8s?.context ?? 'cluster'}`
  return quote?.total_usd != null ? `Create node · ${usd(quote.total_usd)}/mo` : 'Create node'
}

/** Phone cost card: `$12/mo + $1 disk`. */
export function costLine(q: Quote | null): string | null {
  if (!q || q.server_usd === null) return null
  return q.disk_usd === null ? `${usd(q.server_usd)}/mo` : `${usd(q.server_usd)}/mo + ${usd(q.disk_usd)} disk`
}

/** Desktop footer: `$12/mo server + $1/mo disk, billed by DigitalOcean`. */
export function billedLine(q: Quote | null, label: string): string | null {
  if (!q || q.server_usd === null) return null
  const disk = q.disk_usd === null ? '' : ` + ${usd(q.disk_usd)}/mo disk`
  return `${usd(q.server_usd)}/mo server${disk}, billed by ${label}`
}

/** Kubernetes footer: `Uses your cluster · 2 GB request · 10 Gi volume`. */
export function k8sLine(memory_gb: number, volume_gb = DEFAULT_VOLUME_GB): string {
  return `Uses your cluster · ${memory_gb} GB request · ${volume_gb} Gi volume`
}

// ---- names ----

/** `check_name` of `crates/cloudnodes/src/providers/mod.rs`: 1-40 of `[a-z0-9-]`, a letter first, no trailing `-`. */
export function checkName(name: string): string | null {
  const ok = name.length >= 1 && name.length <= 40 && /^[a-z][a-z0-9-]*$/.test(name) && !name.endsWith('-')
  return ok ? null : '1-40 of a-z, 0-9 and -, starting with a letter, not ending in -'
}

/** The user's login, from the engine's data dir (`/Users/sky/…`, `/home/sky/…`, `C:\Users\sky\…`). */
export function userOf(dataDir: string | null | undefined): string | null {
  const m = /(?:^|[\\/])(?:Users|home)[\\/]([^\\/]+)/.exec(dataDir ?? '')
  return m?.[1] ?? null
}

function slug(s: string): string {
  const t = s
    .toLowerCase()
    .normalize('NFKD')
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 33)
    .replace(/-+$/, '')
  return t && /^[a-z]/.test(t) ? t : t ? `n${t}`.slice(0, 33) : ''
}

/** `sky-cloud` (the mockup's default), then `sky-cloud-2`, … past names already taken. */
export function defaultName(user: string | null, taken: readonly string[] = []): string {
  const base = `${slug(user ?? '') || 'my'}-cloud`
  if (!taken.includes(base)) return base
  for (let i = 2; ; i++) if (!taken.includes(`${base}-${i}`)) return `${base}-${i}`
}

/** 🇬🇧 from `GB`. */
export function flag(country: string): string {
  if (!/^[A-Za-z]{2}$/.test(country)) return ''
  return String.fromCodePoint(...[...country.toUpperCase()].map((c) => 0x1f1e6 + c.charCodeAt(0) - 65))
}

// ---- progress ----

/** One `CloudNodeProgress` folded: the latest stage, estimate and lines of a node's long job. */
export type CloudProgress = { stage: Stage | null; pct: number | null; eta_secs: number | null; lines: string[] }
export const LINES_CAP = 200

export function foldProgress(prev: CloudProgress | undefined, e: { stage: Stage | null; line: string; pct: number | null; eta_secs: number | null }): CloudProgress {
  const lines = [...(prev?.lines ?? []), e.line]
  return {
    stage: e.stage ?? prev?.stage ?? null,
    pct: e.pct ?? prev?.pct ?? null,
    eta_secs: e.eta_secs ?? prev?.eta_secs ?? null,
    lines: lines.length > LINES_CAP ? lines.slice(lines.length - LINES_CAP) : lines,
  }
}

/** `About 2 minutes left`, `Less than a minute left`; `null` with no estimate. */
export function etaText(secs: number | null | undefined): string | null {
  if (secs == null) return null
  if (secs < 60) return 'Less than a minute left'
  const m = Math.round(secs / 60)
  return `About ${m} minute${m === 1 ? '' : 's'} left`
}

export type StageStatus = 'done' | 'active' | 'waiting' | 'failed'
export type StageRow = { stage: Stage; label: string; status: StageStatus }

const later = (a: Stage | null, b: Stage | null): Stage | null => (a === null ? b : b === null ? a : STAGES.indexOf(a) >= STAGES.indexOf(b) ? a : b)

/** The progress step's five rows: done before the current stage, active (or failed) at it, waiting after. */
export function stageRows(o: {
  node: Pick<CloudNode, 'state' | 'stage' | 'region_label' | 'region' | 'volume_gb' | 'version' | 'provider' | 'k8s'> | null
  progress: CloudProgress | undefined
  version: string | null
}): StageRow[] {
  const { node, progress, version } = o
  const current = later(node?.stage ?? null, progress?.stage ?? null) ?? 'server'
  const ready = node?.state === 'ready'
  const failed = node?.state === 'failed'
  const at = STAGES.indexOf(current)
  const k8s: K8sOptions | null = node?.k8s ?? null
  const where = node?.provider === 'kubernetes' ? (k8s?.namespace ?? k8s?.context ?? 'your cluster') : (node?.region_label ?? node?.region ?? 'your region')
  const label: Record<Stage, string> = {
    server: node?.provider === 'kubernetes' ? `Deployment created in ${where}` : `Server created in ${where}`,
    storage: `Storage attached (${node?.volume_gb ?? DEFAULT_VOLUME_GB} GB)`,
    install: `Installing theuth${version ? ` ${version}` : ''} (signed)`,
    join: 'Joining your mesh',
    ready: 'Ready',
  }
  return STAGES.map((stage, i) => ({
    stage,
    label: label[stage],
    status: ready ? 'done' : i < at ? 'done' : i > at ? 'waiting' : failed ? 'failed' : stage === 'ready' ? 'done' : 'active',
  }))
}

/** The bar: the job's own estimate, else the share of stages done. */
export function progressPct(rows: readonly StageRow[], progress: CloudProgress | undefined): number {
  if (rows.every((r) => r.status === 'done')) return 100
  if (progress?.pct != null) return Math.max(0, Math.min(100, progress.pct))
  const done = rows.filter((r) => r.status === 'done').length
  return Math.round(((done + 0.5) / rows.length) * 100)
}

// ---- rows ----

export type Tone = 'ok' | 'warn' | 'bad' | 'muted' | 'busy'

/** The row's word for the create job's current stage (`creating · installing`). */
const STAGE_WORD: Record<Stage, string> = { server: 'server', storage: 'storage', install: 'installing', join: 'joining', ready: 'ready' }

/** Settings › Machines status: `online · direct`, `offline`, `creating · installing`, `not on this engine`. */
export function statusOf(n: Pick<CloudNode, 'state' | 'stage' | 'reachable' | 'path'>): { text: string; tone: Tone } {
  switch (n.state) {
    case 'ready':
      if (n.reachable === false || n.path === 'offline') return { text: 'offline', tone: 'bad' }
      return { text: n.path ? `online · ${n.path}` : 'online', tone: 'ok' }
    case 'creating':
      return { text: n.stage ? `creating · ${STAGE_WORD[n.stage]}` : 'creating', tone: 'busy' }
    case 'failed':
      return { text: 'failed', tone: 'bad' }
    case 'busy':
      return { text: 'working…', tone: 'busy' }
    case 'destroying':
      return { text: 'destroying…', tone: 'busy' }
    case 'orphan':
      return { text: 'not on this engine', tone: 'warn' }
    case 'volume_kept':
      return { text: 'destroyed · disk kept', tone: 'muted' }
  }
}

/** Disk price of a node: its volume at the provider's per-GB price. */
export function diskUsd(n: Pick<CloudNode, 'volume_gb'>, perGb: number | null | undefined): number | null {
  if (perGb == null) return null
  return cents(perGb * (n.volume_gb ?? DEFAULT_VOLUME_GB))
}

/**
 * What destroying saves a month: the whole bill, less the disk when it is kept (the provider keeps billing it).
 * `null` without a price (Kubernetes, an orphan whose size the catalog lacks).
 */
export function destroySaving(n: Pick<CloudNode, 'monthly_usd' | 'volume_gb'>, keepDisk: boolean, perGb: number | null | undefined): number | null {
  if (n.monthly_usd == null) return null
  const disk = diskUsd(n, perGb) ?? 0
  return cents(keepDisk ? n.monthly_usd - disk : n.monthly_usd)
}

/** Resize choice: the server at that size plus the node's disk (`4 GB · $25/mo`). */
export function resizePrice(p: ProviderInfo | undefined, n: Pick<CloudNode, 'volume_gb'>, memory_gb: number): number | null {
  if (!p) return null
  return localQuote(p, memory_gb, n.volume_gb ?? DEFAULT_VOLUME_GB)?.total_usd ?? null
}
