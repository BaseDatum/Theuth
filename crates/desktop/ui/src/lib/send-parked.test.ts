import { readFileSync } from 'node:fs'
import { beforeEach, expect, test, vi } from 'vitest'
import { draftSkill } from '@/components/chat/slash'
import type { Api } from '@/rpc/bindings/client'
import { useSkillDraft } from './drafts'
import { sendParked } from './send-parked'
import { activeFor, activeSuffix, useSuffixes } from './suffixes'

vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn(), message: vi.fn() }) }))

// No DOM in the test runner: the smallest `Storage` the stores need.
const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => mem.get(k) ?? null,
  setItem: (k: string, v: string) => void mem.set(k, v),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})

const S = 'sess-1'
const TERSE = { id: 'terse', name: 'Terse', text: 'Answer in one paragraph.' }

type Call = { method: string; session: string; skills: string[]; words: string; files?: string[] }
function engine() {
  const calls: Call[] = []
  // `files` only when the client passed the fourth parameter (no files: the old three-parameter call).
  const call = (method: string, session: string, skills: string[], words: string, rest: unknown[]): Call =>
    rest.length ? { method, session, skills, words, files: rest[0] as string[] } : { method, session, skills, words }
  const api = {
    use_skill: vi.fn(async (session: string, name: string, words: string, ...rest: unknown[]) => (calls.push(call('use_skill', session, [name], words, rest)), session)),
    use_skills: vi.fn(async (session: string, names: string[], words: string, ...rest: unknown[]) => (calls.push(call('use_skills', session, names, words, rest)), session)),
    new_session: vi.fn(async () => 'new-chat'),
  }
  return { api: api as unknown as Api, calls }
}

/** What would ride on `chat`'s next message: parked skills and the active suffix. */
function leftover(chat: string) {
  const s = useSuffixes.getState()
  return { skills: useSkillDraft.getState().by[chat] ?? [], suffix: activeSuffix(s.list, activeFor(s.active[chat], s.always[chat])) }
}

beforeEach(() => {
  mem.clear()
  useSuffixes.setState({ list: [TERSE], active: {}, engine: null, always: {} })
  useSkillDraft.setState({ by: {} })
})

test('skill then suffix: one input carries the skill and the words closed by the suffix; nothing is left for the next message', async () => {
  const { api, calls } = engine()
  useSkillDraft.getState().add(S, 'review')
  useSuffixes.getState().activate(S, TERSE.id, 'once')
  await sendParked(api, S, 'check the parser', S)
  expect(calls).toEqual([{ method: 'use_skill', session: S, skills: ['review'], words: 'check the parser\n\nAnswer in one paragraph.' }])
  expect(leftover(S)).toEqual({ skills: [], suffix: null })
})

test('suffix then skill (several skills): the same single input, in parking order', async () => {
  const { api, calls } = engine()
  useSuffixes.getState().activate(S, TERSE.id, 'once')
  useSkillDraft.getState().add(S, 'review')
  useSkillDraft.getState().add(S, 'rust')
  await sendParked(api, S, 'check the parser', S)
  expect(calls).toEqual([{ method: 'use_skills', session: S, skills: ['review', 'rust'], words: 'check the parser\n\nAnswer in one paragraph.' }])
  expect(leftover(S)).toEqual({ skills: [], suffix: null })
})

test('a session-scoped (always) suffix goes with the skill and keeps applying', async () => {
  const { api, calls } = engine()
  useSuffixes.getState().activate(S, TERSE.id, 'always')
  useSkillDraft.getState().add(S, 'review')
  await sendParked(api, S, 'go', S)
  expect(calls[0]?.words).toBe('go\n\nAnswer in one paragraph.')
  expect(leftover(S)).toEqual({ skills: [], suffix: { ...TERSE, scope: 'always' } })
})

test('a failed send spends no suffix (it is still on for the retry)', async () => {
  const { api } = engine()
  vi.mocked(api.use_skill).mockRejectedValueOnce(new Error('offline'))
  useSuffixes.getState().activate(S, TERSE.id, 'once')
  useSkillDraft.getState().add(S, 'review')
  await expect(sendParked(api, S, 'go', S)).rejects.toThrow('offline')
  expect(leftover(S).suffix?.id).toBe(TERSE.id)
})

test('new chat: suffix on the draft, then a skill (which creates the chat): the first message carries both; the next new chat gets neither', async () => {
  const { api, calls } = engine()
  const navigate = vi.fn(async () => {})
  // The new-chat composer's draft key is '' (`suffixChat` = session ?? draftKey).
  useSuffixes.getState().activate('', TERSE.id, 'once')
  const id = await draftSkill(api, navigate as never, null, 'review', 'check the parser', '')
  expect(id).toBe('new-chat')
  expect(leftover('')).toEqual({ skills: [], suffix: null })
  await sendParked(api, id, 'check the parser', id)
  expect(calls).toEqual([{ method: 'use_skill', session: 'new-chat', skills: ['review'], words: 'check the parser\n\nAnswer in one paragraph.' }])
  expect(leftover(id)).toEqual({ skills: [], suffix: null })
})

test('new chat: skill first (the chat exists), then the suffix on it: both go with the first message', async () => {
  const { api, calls } = engine()
  const id = await draftSkill(api, vi.fn(async () => {}) as never, null, 'review', '', '')
  useSuffixes.getState().activate(id, TERSE.id, 'once')
  await sendParked(api, id, 'go', id)
  expect(calls[0]).toEqual({ method: 'use_skill', session: id, skills: ['review'], words: 'go\n\nAnswer in one paragraph.' })
  expect(leftover(id)).toEqual({ skills: [], suffix: null })
})

test('skill, suffix and files: one call carries all three; nothing is left for the next message', async () => {
  const { api, calls } = engine()
  useSkillDraft.getState().add(S, 'review')
  useSuffixes.getState().activate(S, TERSE.id, 'once')
  await sendParked(api, S, 'what is wrong here?', S, ['/tmp/a/shot.png', '/tmp/a/log.pdf'])
  expect(calls).toEqual([
    { method: 'use_skill', session: S, skills: ['review'], words: 'what is wrong here?\n\nAnswer in one paragraph.', files: ['/tmp/a/shot.png', '/tmp/a/log.pdf'] },
  ])
  expect(leftover(S)).toEqual({ skills: [], suffix: null })
})

test('several skills with files: one use_skills call with the files; no files keeps the three-parameter call', async () => {
  const { api, calls } = engine()
  useSkillDraft.getState().add(S, 'review')
  useSkillDraft.getState().add(S, 'rust')
  await sendParked(api, S, 'go', S, ['/tmp/a/shot.png'])
  useSkillDraft.getState().add(S, 'review')
  await sendParked(api, S, 'again', S)
  expect(calls).toEqual([
    { method: 'use_skills', session: S, skills: ['review', 'rust'], words: 'go', files: ['/tmp/a/shot.png'] },
    { method: 'use_skill', session: S, skills: ['review'], words: 'again' },
  ])
})

test('the composer sends parked skills with their files through sendParked, clears the box only after, and refuses nothing', () => {
  const src = readFileSync(new URL('../components/chat/Composer.tsx', import.meta.url), 'utf8')
  expect(src).toContain('await sendParked(api, session, pinned ? compose([pinned], words) : words, suffixChat, files)\n        reset()')
  expect(src).not.toContain('Files cannot go with a skill')
  expect(src).toContain('draftSkill(api, navigate, null, name, text, suffixChat)')
  expect(src).toContain("draftSkill(api, navigate, session, name, words.join(' '), suffixChat)")
  expect(src).not.toContain('sendSkills(api')
})
