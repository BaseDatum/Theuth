import { expect, test } from 'vitest'
import type { ResumeBanner } from '@/store/sessions'
import { offersResume, resumeText } from './ResumeBanner'

const at = Date.UTC(2026, 9, 6, 12, 5)
const hhmm = new Date(at).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })
const base: ResumeBanner = { turn: 't', kind: 'interrupted', at, resumed: false, here: true }
const report = { replayed: 2, skipped: ['git push'], lostJobs: 0 }

test('resumeText: 109 §8.2 wording by kind and stage; the counts fill in once the report lands', () => {
  expect(resumeText(base)).toBe(`Interrupted by a restart at ${hhmm} — resuming…`)
  expect(resumeText({ ...base, resumed: true })).toBe(`Interrupted by a restart at ${hhmm} — resumed automatically`)
  expect(resumeText({ ...base, resumed: true, report })).toBe(
    `Interrupted by a restart at ${hhmm} — resumed automatically (2 tool calls replayed, 1 skipped: git push, effect unknown)`,
  )
  expect(resumeText({ ...base, resumed: true, report: { replayed: 1, skipped: [], lostJobs: 1 } })).toBe(
    `Interrupted by a restart at ${hhmm} — resumed automatically (1 tool call replayed, 1 job lost)`,
  )
  expect(resumeText({ ...base, kind: 'suspended' })).toBe(`Paused for an update at ${hhmm} — resumes after the restart`)
  expect(resumeText({ ...base, kind: 'suspended', resumed: true })).toBe(`Paused for an update at ${hhmm} — resumed`)
  // 139: the boot causes keep today's strings.
  for (const cause of ['crash', 'shutdown', 'update'] as const) expect(resumeText({ ...base, cause })).toBe(`Interrupted by a restart at ${hhmm} — resuming…`)
})

test('resumeText: 139 §9.3 — a replica names who runs the turn, never "resuming…"', () => {
  const label = (e: string) => e.toUpperCase()
  const theirs = { ...base, here: false, owner: 'a' }
  expect(resumeText(theirs, label)).toBe('Running on A')
  // `resumed` is the owner's flag: a relayed TurnResumed still reads as running there.
  expect(resumeText({ ...theirs, kind: 'suspended', resumed: true, cause: 'transfer' }, label)).toBe('Running on A')
  expect(resumeText({ ...theirs, kind: 'suspended', cause: 'transfer', owner: 'b' }, label)).toBe('Moving to B…')
  expect(resumeText({ ...theirs, kind: 'suspended', cause: 'shutdown' }, label)).toBe('Paused on A — resumes there')
  expect(resumeText({ ...theirs, kind: 'suspended' }, label)).toBe('Paused on A — resumes there')
  expect(resumeText({ ...theirs, owner: undefined })).toBe('Running on another machine')
  expect(resumeText({ ...theirs, report }, label)).toBe('Running on A')
})

test('resumeText: 139 §9.3 — the owner side of a transfer / takeover, with the counts', () => {
  const label = (e: string) => e.toUpperCase()
  const taken = { ...base, cause: 'transfer' as const, from: 'a', resumed: true }
  // 183 R1 (H3): a move reads "Moved here"; "Taken from" is the takeover's (below).
  expect(resumeText(taken, label)).toBe(`Moved here from A at ${hhmm} — resumed here`)
  expect(resumeText({ ...taken, report }, label)).toBe(`Moved here from A at ${hhmm} — resumed here (2 tool calls replayed, 1 skipped: git push, effect unknown)`)
  expect(resumeText({ ...taken, from: undefined })).toBe(`Moved here at ${hhmm} — resumed here`)
  // Before the resume: the releasing owner (TurnSuspended{release}) and a taker under `resume: ask` share the line.
  expect(resumeText({ ...taken, resumed: false, kind: 'suspended' }, label)).toBe(`Paused for a transfer at ${hhmm} — resumes on the new owner`)
  const takeover = { ...taken, cause: 'takeover' as const }
  expect(resumeText(takeover, label)).toBe(`Taken from A at ${hhmm} — resumed here; A may still have been working, its unshipped work was discarded`)
  expect(resumeText({ ...takeover, report: { replayed: 1, skipped: [], lostJobs: 2 } }, label)).toBe(
    `Taken from A at ${hhmm} — resumed here; A may still have been working, its unshipped work was discarded (1 tool call replayed, 2 jobs lost)`,
  )
  expect(resumeText({ ...takeover, resumed: false }, label)).toBe(`Taken from A at ${hhmm} — resumes when you continue here`)
  // The Resume button shows only for that last state.
  expect(offersResume({ ...takeover, resumed: false })).toBe(true)
  expect(offersResume(takeover)).toBe(false)
  expect(offersResume({ ...takeover, resumed: false, here: false })).toBe(false)
  expect(offersResume({ ...taken, resumed: false })).toBe(false)
  expect(offersResume(base)).toBe(false)
})
