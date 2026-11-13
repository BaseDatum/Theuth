// 57 §7.1: Settings › Learning › Decision models (was Labs) — the text and reducers behind the three cards, kept pure so `bun run test` covers them.

import { fmtBytes } from '@/lib/files'
import type { FamilyStats } from "@/rpc/bindings/method.dreaming.output"
import type { LoadedInfo } from "@/rpc/bindings/method.dreaming.output"
import type { Mode } from "@/rpc/bindings/method.dreaming.output"
import type { ModeInfo } from "@/rpc/bindings/method.dreaming.output"
import type { TrainingInfo } from "@/rpc/bindings/method.dreaming.output"

/** The decision-family prefixes Labs manages; also the `decisions/training/<prefix>/` directory names. */
export type Prefix = 'meeting' | 'browser'

/** One help line per switch position (57 §3.4 for meeting, §4.1 for browser); also the items' tooltips. */
export const MODE_HELP: Record<Prefix, Record<Mode, string>> = {
  meeting: {
    off: 'Off: every transcript batch is a model turn, as before.',
    shadow: 'Shadow: the oracle is asked and its verdict shown, the model turn still runs; the session rows show whether they agreed.',
    active: 'Active: the oracle gates the turn — a confident "nothing to look up" skips it, a confident cached brief is published directly.',
  },
  browser: {
    off: 'Off: browser_run is not offered to the assistant (takes effect at the next daemon start).',
    shadow: 'Shadow: browser_run decides one step, shows it, and hands the page back — nothing is clicked or typed.',
    active: 'Active: browser_run performs up to 25 actions (60 s) in the headless browser under one approval, no model turn per step.',
  },
}

/** 57 §7.1: the consent text under "Share with theuth". */
export const SHARE_CONSENT =
  'Uploads are byte-identical to the rows on this disk: no session, meeting or account identifiers, secrets already replaced by placeholders. At most one batch per node per day. Turning this off stops uploads but cannot recall what was already sent.'

/** Shown while no uploader exists: the switch is saved and honoured later. */
export const SHARE_UNAVAILABLE = 'Upload not available in this version — your choice is saved and honoured by the uploader once it ships.'

/** What one captured row holds, in the family's own words (57 §7.1). */
export const CAPTURE_ROW: Record<Prefix, string> = {
  meeting:
    "One row per transcript batch the oracle handled: the last transcript batch — other people's speech — the classifier's numbers, what the assistant did next, and whether you tapped or dismissed the brief.",
  browser:
    "One row per browser_run step: the page's element list and visible text, the goal, and the action taken. This version's browser_run does not write rows yet; the switch is saved for the one that does.",
}

/**
 * Why the mode control is disabled, or `undefined` when it works. A missing non-default checkpoint names the
 * fetch script (57 §7.1: the Download action waits on the release bucket listing `decision-browser/`).
 */
export function blockedReason(info: ModeInfo | undefined, prefix: Prefix, error?: string): string | undefined {
  if (error) return error
  if (!info || info.effective === info.mode || info.mode === 'off') return undefined
  if (!info.enabled) return info.reason ?? 'the decision plugin is disabled'
  if (!info.installed && prefix === 'browser')
    return 'the decision-browser checkpoint is not installed. Run scripts/fetch-laya-browser.sh (0.7 GB, cklxx/laya-browser v17s, into ~/.theuth/models/decision-browser); once it is on the release bucket the app downloads it at start.'
  return info.reason ?? `running as ${info.effective}`
}

const sum = (xs: FamilyStats[], f: (s: FamilyStats) => number): number => xs.reduce((n, s) => n + f(s), 0)

/** Live counters for one card, folded from `decision_stats` (57 §7.1: batches · skipped · briefs · tier-2 calls · agreed %). */
export function countersLine(stats: FamilyStats[] | undefined, prefix: Prefix): string {
  const under = (stats ?? []).filter((s) => s.family.startsWith(`${prefix}.`))
  if (under.length === 0) return 'no decisions today'
  const of = (family: string) => under.find((s) => s.family === `${prefix}.${family}`)
  const confirmed = sum(under, (s) => s.confirmed)
  const corrected = sum(under, (s) => s.corrected)
  const agreed = confirmed + corrected > 0 ? `agreed ${Math.round((100 * confirmed) / (confirmed + corrected))} %` : 'no outcomes yet'
  const tier2 = `${sum(under, (s) => s.tier2)} tier-2 calls`
  if (prefix === 'meeting')
    return [`${of('kind')?.asks ?? 0} batches`, `${of('kind')?.accepted ?? 0} skipped`, `${of('brief')?.accepted ?? 0} briefs`, tier2, agreed].join(' · ')
  const op = of('operation')
  return [`${op?.asks ?? 0} steps`, `${op?.accepted ?? 0} acted`, `${of('target')?.accepted ?? 0} targets`, tier2, agreed].join(' · ')
}

/** `3 today · 41 total · 84 KB` for the capture row. */
export function captureLine(t: TrainingInfo | undefined): string {
  if (!t) return '…'
  return `${t.examples_today} today · ${t.examples_total} total · ${fmtBytes(Number(t.bytes))}`
}

/** The Share switch: off and disabled while capture is off (57 §7.1 — consent covers what capture writes). */
export function shareState(t: TrainingInfo | undefined): { checked: boolean; disabled: boolean } {
  if (!t) return { checked: false, disabled: true }
  return { checked: t.enabled && t.share, disabled: !t.enabled }
}

/** 57 §2.1 item 4 / §7.1 header: `none` / `meeting · 903 MB · 4 min` (`default` is the meeting checkpoint; `rss_mb` is the whole daemon's footprint). */
export function loadedText(infos: LoadedInfo[] | undefined): string {
  if (!infos || infos.length === 0) return 'none'
  return infos
    .map((i) => {
      const name = i.checkpoint === 'default' ? 'meeting' : i.checkpoint
      const secs = Number(i.since_secs)
      const since = secs < 60 ? `${secs} s` : `${Math.floor(secs / 60)} min`
      return `${name} · ${i.rss_mb} MB · ${since}`
    })
    .join(' / ')
}
