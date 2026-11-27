// 224: the cloud-node UI's pure helpers — prices, names, statuses, the create job's stage rows, the destroy saving.
// No React, no RPC: the wizard, Settings › Machines and the tests share them. No price lives here: every number comes
// from the provider at runtime (`cloud_sizes`, `cloud_list`'s `monthly_usd`, `ProviderInfo.volume_usd_per_gb`).

import { type CloudNode, DEFAULT_VOLUME_GB, type K8sOptions, type NodeSpec, type ProviderInfo, type ProviderKind, type Quote, type SizeList, type SizeOption, STAGES, type Stage } from './types'

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

/** The providers a screen offers: Kubernetes (`desktop_only`) never on a phone. */
export function offered(providers: readonly ProviderInfo[], phone: boolean): ProviderInfo[] {
  return providers.filter((p) => !(phone && p.desktop_only))
}

/** A size list carries prices: `priced` and every server price present. */
export function pricedSizes(list: SizeList | undefined): boolean {
  return !!list && list.priced && !list.error && list.sizes.every((s) => s.monthly_usd !== null)
}

/**
 * The first-run chip: the cheapest smallest server across connected, non-beta providers whose live prices are already
 * fetched (`from $12/mo`); `null` before any sign-in or price — no number is ever shown that the provider did not give.
 */
export function fromPrice(providers: readonly ProviderInfo[], phone: boolean, lists: Partial<Record<ProviderKind, SizeList>>): string | null {
  const prices = offered(providers, phone)
    .filter((p) => p.available && p.connected && !p.beta && pricedSizes(lists[p.provider]))
    .flatMap((p) => {
      const smallest = [...(lists[p.provider]?.sizes ?? [])].sort((a, b) => a.memory_gb - b.memory_gb)[0]
      return smallest?.monthly_usd != null ? [smallest.monthly_usd] : []
    })
  if (!prices.length) return null
  return `from $${Math.round(Math.min(...prices))}/mo`
}

/** The monthly cost of `memory_gb` from the live sizes plus the disk (`catalog::quote`'s arithmetic); prices `null` when unpriced. */
export function localQuote(kind: ProviderKind, sizes: readonly SizeOption[] | undefined, perGb: number | null, memory_gb: number, volume_gb = DEFAULT_VOLUME_GB): Quote | null {
  const size = sizes?.find((s) => s.memory_gb === memory_gb)
  if (!size) return null
  const disk = perGb === null ? null : cents(perGb * volume_gb)
  const total = size.monthly_usd !== null && disk !== null ? cents(size.monthly_usd + disk) : null
  return { provider: kind, size_slug: size.slug, server_usd: size.monthly_usd, disk_usd: disk, total_usd: total }
}

/** The create button: the press is the approval, so it names the monthly total (`Create node · $13/mo`). */
export function createLabel(spec: Pick<NodeSpec, 'provider' | 'k8s'>, quote: Quote | null): string {
  if (spec.provider === 'kubernetes') return `Deploy to ${spec.k8s?.context ?? 'cluster'}`
  return quote?.total_usd != null ? `Create node · ${usd(quote.total_usd)}/mo` : 'Create node'
}

// ---- desktop and size ----

/** Why a provider cannot have the watchable desktop yet (its container image has none); `null` = it can. */
export function desktopBlocked(kind: ProviderKind): string | null {
  if (kind === 'fly') return 'Desktop: coming soon on Fly.io'
  if (kind === 'kubernetes') return 'Desktop: coming soon on Kubernetes'
  return null
}

/** The size the wizard starts at until the user picks one: 4 GB with the desktop, the 2 GB minimum without. */
export const defaultMemory = (desktop: boolean): number => (desktop ? 4 : 2)

/** What each size is for (the size cards). */
export const SIZE_USE: Record<number, string> = {
  2: 'Chat, coding, schedules and routines. Light browsing.',
  4: 'Heavy computer use and browsing.',
  8: 'Several agents at once, big repos, local tools.',
}

export const DESKTOP_NOTE = 'We recommend 4 GB for computer use and browsing.'

/** The line under the cost: `No open ports · updates itself · watchable desktop`. */
export const summaryLine = (desktop: boolean): string => `No open ports · updates itself${desktop ? ' · watchable desktop' : ''}`

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

/** The live sizes with their prices only when the provider gave them (`priced`), else the same sizes unpriced. */
export function sizesOf(list: SizeList | undefined): SizeOption[] | undefined {
  if (!list) return undefined
  return pricedSizes(list) ? list.sizes : list.sizes.map((s) => ({ ...s, monthly_usd: null }))
}
