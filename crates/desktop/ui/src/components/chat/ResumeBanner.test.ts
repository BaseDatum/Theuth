import { expect, test } from 'vitest'
import type { ResumeBanner } from '@/store/sessions'
import { resumeText } from './ResumeBanner'

const at = Date.UTC(2026, 9, 6, 12, 5)
const hhmm = new Date(at).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })
const base: ResumeBanner = { turn: 't', kind: 'interrupted', at, resumed: false }

test('resumeText: 109 §8.2 wording by kind and stage; the counts fill in once the report lands', () => {
  expect(resumeText(base)).toBe(`Interrupted by a restart at ${hhmm} — resuming…`)
  expect(resumeText({ ...base, resumed: true })).toBe(`Interrupted by a restart at ${hhmm} — resumed automatically`)
  expect(resumeText({ ...base, resumed: true, report: { replayed: 2, skipped: ['git push'], lostJobs: 0 } })).toBe(
    `Interrupted by a restart at ${hhmm} — resumed automatically (2 tool calls replayed, 1 skipped: git push, effect unknown)`,
  )
  expect(resumeText({ ...base, resumed: true, report: { replayed: 1, skipped: [], lostJobs: 1 } })).toBe(
    `Interrupted by a restart at ${hhmm} — resumed automatically (1 tool call replayed, 1 job lost)`,
  )
  expect(resumeText({ ...base, kind: 'suspended' })).toBe(`Paused for an update at ${hhmm} — resumes after the restart`)
  expect(resumeText({ ...base, kind: 'suspended', resumed: true })).toBe(`Paused for an update at ${hhmm} — resumed`)
})
