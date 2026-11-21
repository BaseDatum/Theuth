import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { beforeEach, expect, test, vi } from 'vitest'
import { sendSkills, useSkillDraft } from '@/lib/drafts'
import { classify } from '@/lib/injected'
import type { DiscoveryDocument } from '@/rpc/discovery'
import type { SkillMeta } from '@/rpc/bindings/method.agent.output'
import { useConnection } from '@/store/connection'
import { placeholderFor } from './Composer'
import { SkillChips, SkillPicker, SkillsEntry, filterSkills } from './SkillPicker'
import { draftSkill } from './slash'
import type * as Connection from '@/store/connection'

// A static render reads a zustand hook's initial state; read the live one so `engine` takes effect.
vi.mock('@/store/connection', async (original) => {
  const real = await original<typeof Connection>()
  // The scoped store's statics are non-enumerable getters: copy their descriptors, not their values.
  const live = Object.defineProperties((pick: (s: unknown) => unknown) => pick(real.useConnection.getState()), Object.getOwnPropertyDescriptors(real.useConnection))
  return { ...real, useConnection: live }
})

const skill = (name: string, description: string) => ({ name, description }) as SkillMeta
const INDEX = [skill('caveman', 'Speak short'), skill('ponytail', 'Tie long answers back'), skill('deploy', 'Ship the build')]

/** An engine whose discovery document lists exactly `methods`. */
function engine(methods: string[]) {
  const document = { methods: methods.map((name) => ({ name, params: [], result: {} })), 'x-theuth': { subscriptions: [], events: [] } } as unknown as DiscoveryDocument
  useConnection.setState({ discovery: { phase: 'ready', capabilities: null, document, unavailable: new Set() } })
}

const entry = () => renderToStaticMarkup(createElement(SkillsEntry, { onOpen: () => {} }))
const chips = () => {
  const html = renderToStaticMarkup(createElement(SkillChips, { names: useSkillDraft.getState().by.s ?? [], onRemove: () => {} }))
  return [...html.matchAll(/data-skill-chip="([^"]+)"/g)].map((m) => m[1])
}

beforeEach(() => useSkillDraft.setState({ by: {} }))

test('the + menu lists Skills only when the engine has list_skills and use_skill', () => {
  engine(['list_skills', 'use_skill'])
  expect(entry()).toContain('Skills')
  engine(['list_skills'])
  expect(entry()).toBe('')
  engine(['use_skill'])
  expect(entry()).toBe('')
})

test('the picker shows each skill with its description, ticks parked ones, and searches name and description', () => {
  const html = renderToStaticMarkup(createElement(SkillPicker, { skills: INDEX, parked: ['ponytail'], onPick: () => {}, onBack: () => {} }))
  for (const s of INDEX) expect(html).toContain(s.description)
  expect(html).toMatch(/aria-checked="true"[^>]*data-skill="ponytail"/)
  expect(html).toMatch(/aria-checked="false"[^>]*data-skill="caveman"/)
  expect(filterSkills(INDEX, 'SHIP').map((s) => s.name)).toEqual(['deploy'])
  expect(filterSkills(INDEX, 'long back').map((s) => s.name)).toEqual(['ponytail'])
  expect(filterSkills(INDEX, '')).toHaveLength(3)
})

test('picking two skills gives two chips in order; a repeat pick adds nothing; one ✕ removes only its skill', () => {
  const { add, remove } = useSkillDraft.getState()
  add('s', 'ponytail')
  add('s', 'caveman')
  add('s', 'ponytail')
  expect(chips()).toEqual(['ponytail', 'caveman'])
  expect(placeholderFor({ running: false, exports: false, skill: chips().join(', ') })).toBe('Words for ponytail, caveman (optional)')
  remove('s', 'ponytail')
  expect(chips()).toEqual(['caveman'])
  remove('s', 'caveman')
  expect(useSkillDraft.getState().by).toEqual({})
})

test('`/skill:<name>` adds to the parked list instead of replacing it', async () => {
  useSkillDraft.getState().add('s', 'ponytail')
  const went: unknown[] = []
  await draftSkill({} as never, (async (to: unknown) => void went.push(to)) as never, 's', 'caveman', 'go')
  expect(chips()).toEqual(['ponytail', 'caveman'])
  expect(went).toEqual([{ to: '/c/$session', params: { session: 's' } }])
})

test('one skill sends use_skill unchanged; several send one use_skills in order', async () => {
  const calls: unknown[][] = []
  const api = {
    use_skill: async (...a: unknown[]) => (calls.push(['use_skill', ...a]), 's'),
    use_skills: async (...a: unknown[]) => (calls.push(['use_skills', ...a]), 's'),
  } as unknown as Parameters<typeof sendSkills>[0]
  await sendSkills(api, 's', ['caveman'], 'lite')
  await sendSkills(api, 's', ['ponytail', 'caveman'], 'go now')
  expect(calls).toEqual([
    ['use_skill', 's', 'caveman', 'lite'],
    ['use_skills', 's', ['ponytail', 'caveman'], 'go now'],
  ])
})

test('a several-skill input is one card naming every skill in order, then the words', () => {
  const text = '<skill name="ponytail">\nTie.\n</skill>\n\n<skill name="caveman">\nGrunt.\n</skill>\n\ngo now'
  expect(classify(text)).toEqual([{ kind: 'skill', name: 'ponytail, caveman', words: 'go now' }])
  expect(classify('<skill name="x">\nb\n</skill>\n\nw')).toEqual([{ kind: 'skill', name: 'x', words: 'w' }])
})
