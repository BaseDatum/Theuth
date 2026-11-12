import { expect, test } from 'vitest'
import { blockedReason, captureLine, countersLine, loadedText, shareState } from './labs'
import type { FamilyStats } from "@/rpc/bindings/method.dreaming.output"
import type { ModeInfo } from "@/rpc/bindings/method.dreaming.output"
import type { TrainingInfo } from "@/rpc/bindings/method.dreaming.output"

// ts-rs types u64 as `bigint`; the wire carries numbers, so the fixtures do too.
const info = (p: Record<string, unknown>): ModeInfo =>
  ({ mode: 'shadow', effective: 'shadow', reason: null, enabled: true, installed: true, budget_ms: 400, escalate_budget_ms: 3000, ...p }) as unknown as ModeInfo

test('blockedReason: nothing when effective matches or the switch is off; disabled and missing checkpoint name themselves', () => {
  expect(blockedReason(info({}), 'meeting')).toBeUndefined()
  expect(blockedReason(info({ mode: 'off', effective: 'off' }), 'meeting')).toBeUndefined()
  expect(blockedReason(undefined, 'meeting')).toBeUndefined()
  expect(blockedReason(info({}), 'meeting', 'socket closed')).toBe('socket closed')
  expect(blockedReason(info({ effective: 'off', enabled: false, reason: 'the decision plugin is disabled (decision.enabled: false)' }), 'meeting')).toContain('disabled')
  // The daemon's reason for the default checkpoint is shown as is …
  expect(blockedReason(info({ effective: 'off', installed: false, reason: 'model missing: model.safetensors not in /m' }), 'meeting')).toBe(
    'model missing: model.safetensors not in /m',
  )
  // … the browser checkpoint gets the fetch instructions (no Download action until the bucket lists it).
  expect(blockedReason(info({ effective: 'off', installed: false, reason: 'checkpoint missing: decision-browser — …' }), 'browser')).toMatch(
    /fetch-laya-browser\.sh/,
  )
})

const stat = (family: string, p: Record<string, number>): FamilyStats =>
  ({ family, mode: 'shadow', asks: 0, tier2: 0, tier2_accepted: 0, accepted: 0, escalations: {}, p50_ms: 0, p95_ms: 0, confirmed: 0, corrected: 0, unknown: 0, agreement: null, ...p }) as unknown as FamilyStats

test('countersLine folds the prefix families; other prefixes are ignored', () => {
  const stats = [
    stat('meeting.kind', { asks: 14, accepted: 9, tier2: 2, confirmed: 8, corrected: 1 }),
    stat('meeting.brief', { asks: 14, accepted: 3, tier2: 1, confirmed: 3 }),
    stat('meeting.subject', { asks: 5 }),
    stat('browser.operation', { asks: 7, accepted: 6, confirmed: 2, corrected: 2 }),
    stat('browser.target', { asks: 7, accepted: 4 }),
  ]
  expect(countersLine(stats, 'meeting')).toBe('14 batches · 9 skipped · 3 briefs · 3 tier-2 calls · agreed 92 %')
  expect(countersLine(stats, 'browser')).toBe('7 steps · 6 acted · 4 targets · 0 tier-2 calls · agreed 50 %')
  expect(countersLine([stat('meeting.kind', { asks: 2 })], 'meeting')).toBe('2 batches · 0 skipped · 0 briefs · 0 tier-2 calls · no outcomes yet')
  expect(countersLine(undefined, 'browser')).toBe('no decisions today')
  expect(countersLine(stats.slice(0, 3), 'browser')).toBe('no decisions today')
})

const training = (p: Record<string, unknown>): TrainingInfo =>
  ({ enabled: false, share: false, examples_today: 0, examples_total: 0, bytes: 0, dir: '/d/decisions/training/meeting', ...p }) as unknown as TrainingInfo

test('shareState: Share is off and locked while Capture is off, and never shown on without it', () => {
  expect(shareState(undefined)).toEqual({ checked: false, disabled: true })
  expect(shareState(training({}))).toEqual({ checked: false, disabled: true })
  expect(shareState(training({ share: true }))).toEqual({ checked: false, disabled: true })
  expect(shareState(training({ enabled: true }))).toEqual({ checked: false, disabled: false })
  expect(shareState(training({ enabled: true, share: true }))).toEqual({ checked: true, disabled: false })
})

test('captureLine', () => {
  expect(captureLine(undefined)).toBe('…')
  expect(captureLine(training({ examples_today: 3, examples_total: 41, bytes: 86016 }))).toBe('3 today · 41 total · 84 KB')
})

test('loadedText: none at idle; `default` reads as meeting; seconds under a minute, minutes after', () => {
  expect(loadedText(undefined)).toBe('none')
  expect(loadedText([])).toBe('none')
  expect(loadedText([{ checkpoint: 'default', since_secs: 4, rss_mb: 1169 }])).toBe('meeting · 1169 MB · 4 s')
  expect(
    loadedText([
      { checkpoint: 'default', since_secs: 250, rss_mb: 1903 },
      { checkpoint: 'browser', since_secs: 61, rss_mb: 1903 },
    ]),
  ).toBe('meeting · 1903 MB · 4 min / browser · 1903 MB · 1 min')
})
