import { beforeEach, expect, test, vi } from 'vitest'
import type { Api } from '@/rpc/bindings/client'
import { EMPTY, NO_PICK, PLAIN_OPTIONS, load, openDraftSession, sendFirst, useDraftTarget, useDrafts } from './drafts'
import { attach, attachRef } from './pastes'

// No DOM in the test runner: the smallest `Storage` the store needs (the module's own `load()` ran without one and
// read as empty, which is the corrupt/absent-storage path).
const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => mem.get(k) ?? null,
  setItem: (k: string, v: string) => void mem.set(k, v),
  clear: () => mem.clear(),
})

const ref = { kind: 'note' as const, id: 'n1', title: 'Plan', meta: '' }
const held = attachRef(ref, attach('/tmp/a.png', 4, 4, EMPTY.pastes))

beforeEach(() => {
  localStorage.clear()
  useDrafts.setState({ by: {}, edits: {}, pick: NO_PICK, settings: {}, opening: null })
  useDraftTarget.getState().set({ engine: null, cwd: null, reuse: null })
})

test('draft → edit → cancel leaves the composer draft intact (text, pastes, attachments)', () => {
  const s = useDrafts.getState()
  s.set('c1', { text: 'half a thought ⟦paste 1⟧', pastes: held })
  s.beginEdit('c1', { turn: 3, text: 'sent words', refs: [ref] })
  s.updateEdit('c1', { text: 'sent words, changed', refs: [] })
  expect(useDrafts.getState().edits.c1).toEqual({ turn: 3, text: 'sent words, changed', refs: [] })
  s.endEdit('c1')
  expect(useDrafts.getState().edits.c1).toBeUndefined()
  expect(useDrafts.getState().by.c1).toEqual({ text: 'half a thought ⟦paste 1⟧', pastes: held })
})

test('one draft per session and one for the new chat', () => {
  const s = useDrafts.getState()
  s.setText('', 'new chat words')
  s.setText('c1', 'chat one')
  s.setPastes('c1', (p) => attachRef(ref, p))
  expect(useDrafts.getState().by['']).toEqual({ text: 'new chat words', pastes: EMPTY.pastes })
  expect(useDrafts.getState().by.c1).toEqual({ text: 'chat one', pastes: attachRef(ref, EMPTY.pastes) })
})

test('reload restores drafts and an open edit; an empty draft is not stored', () => {
  const s = useDrafts.getState()
  s.set('c1', { text: 'kept', pastes: held })
  s.setText('c2', '')
  s.beginEdit('c1', { turn: 2, text: 'editing', refs: [ref] })
  const back = load()
  expect(back.by.c1).toEqual({ text: 'kept', pastes: held })
  expect(back.by.c1?.pastes).toBeInstanceOf(Map)
  expect(back.by.c2).toBeUndefined()
  expect(back.edits).toEqual({ c1: { turn: 2, text: 'editing', refs: [ref] } })
})

test('a send keeps the text until acked; the ack drops exactly what was sent', () => {
  const s = useDrafts.getState()
  s.set('c1', { text: 'long thought ⟦paste 1⟧', pastes: held })
  s.sending('c1', 'id-1')
  // In flight: still in the box, flagged, and persisted so a reload mid-send shows the banner.
  expect(useDrafts.getState().by.c1).toEqual({ text: 'long thought ⟦paste 1⟧', pastes: held, unsent: 'id-1' })
  expect(load().by.c1?.unsent).toBe('id-1')
  // Typed during the flight: only the sent text leaves.
  s.setText('c1', 'long thought ⟦paste 1⟧ and more')
  s.acked('c1', 'long thought ⟦paste 1⟧')
  expect(useDrafts.getState().by.c1).toEqual({ text: ' and more', pastes: EMPTY.pastes })
  s.sending('c1', 'id-2')
  s.acked('c1', ' and more')
  expect(useDrafts.getState().by.c1).toEqual({ text: '', pastes: EMPTY.pastes })
  expect(load().by.c1).toBeUndefined()
})

test('a failed send leaves the flagged draft through edits; emptying the box drops the flag', () => {
  const s = useDrafts.getState()
  s.set('c1', { text: 'kept', pastes: held })
  s.sending('c1', 'id-1')
  // Failure changes nothing in the store: the composer reads `unsent` for the banner and the retry id.
  expect(useDrafts.getState().by.c1).toEqual({ text: 'kept', pastes: held, unsent: 'id-1' })
  s.setText('c1', 'kept, edited')
  expect(useDrafts.getState().by.c1?.unsent).toBe('id-1')
  s.setText('c1', '')
  // Chips still held: there is still something to retry.
  expect(useDrafts.getState().by.c1).toEqual({ text: '', pastes: held, unsent: 'id-1' })
  s.set('c1', EMPTY)
  expect(useDrafts.getState().by.c1).toEqual({ text: '', pastes: EMPTY.pastes })
})

test('a corrupt store reads as empty', () => {
  localStorage.setItem('composer-drafts', '{not json')
  expect(load()).toEqual({ by: {}, edits: {}, pick: NO_PICK, settings: {}, opening: null })
})

// 148 R1: the one seam every draft-born session goes through (the composer's send, Talk, `/<skill>`, a parked skill).
const opened = { session: 's-new', retention: 'durable' }
const apiFor = () => {
  const create_session = vi.fn(async () => opened)
  const new_session = vi.fn(async () => 's-plain')
  return { api: { create_session, new_session } as unknown as Api, create_session, new_session }
}

test('148 R2: the draft pick lives with the `\'\'` draft text — persisted in the same blob, back after a remount/reload', () => {
  const s = useDrafts.getState()
  s.setText('', 'half a thought')
  s.setPick({ model: 'openai/gpt-5.4' })
  s.setPick({ effort: 'high' })
  expect(useDrafts.getState().pick).toEqual({ model: 'openai/gpt-5.4', effort: 'high' })
  // A route round-trip remounts NewChat; a relaunch or another window's save re-reads the store: both keep the pick.
  useDrafts.setState(load())
  expect(useDrafts.getState().by['']).toEqual({ text: 'half a thought', pastes: EMPTY.pastes })
  expect(useDrafts.getState().pick).toEqual({ model: 'openai/gpt-5.4', effort: 'high' })
  // A blob saved before the pick existed reads as no pick.
  localStorage.setItem('composer-drafts', JSON.stringify({ drafts: {}, edits: {} }))
  expect(load().pick).toEqual(NO_PICK)
})

test('148 R1: a draft model/effort pick opens through create_session with it (never new_session) and is spent', async () => {
  const { api, create_session, new_session } = apiFor()
  useDrafts.getState().setPick({ model: 'openai/gpt-5.4', effort: 'high' })
  const r = await openDraftSession(api)
  expect(create_session).toHaveBeenCalledWith({ ...PLAIN_OPTIONS, model: 'openai/gpt-5.4', effort: 'high' })
  expect(new_session).not.toHaveBeenCalled()
  expect(r).toEqual({ id: 's-new', open: opened, pick: { model: 'openai/gpt-5.4', effort: 'high' }, spend: expect.any(Function) })
  // Spent in the store and in storage: the next draft starts on the engine default.
  expect(useDrafts.getState().pick).toEqual(NO_PICK)
  expect(load().pick).toEqual(NO_PICK)
  // Effort alone is enough: `new_session()` cannot carry it.
  useDrafts.getState().setPick({ effort: 'low' })
  await openDraftSession(api)
  expect(create_session).toHaveBeenLastCalledWith({ ...PLAIN_OPTIONS, effort: 'low' })
  expect(useDrafts.getState().pick.effort).toBeNull()
})

test('148 R1: no pick and no switch → new_session(); a switch (the composer\'s options) → create_session with them', async () => {
  const { api, create_session, new_session } = apiFor()
  expect(await openDraftSession(api)).toEqual({ id: 's-plain', open: null, pick: NO_PICK, spend: expect.any(Function) })
  expect(create_session).not.toHaveBeenCalled()
  const options = { ...PLAIN_OPTIONS, project: 'lab', model: 'openai/gpt-5.4' }
  useDrafts.getState().setPick({ model: 'openai/gpt-5.4' })
  expect((await openDraftSession(api, options)).id).toBe('s-new')
  expect(create_session).toHaveBeenCalledWith(options)
  expect(new_session).toHaveBeenCalledTimes(1)
  expect(useDrafts.getState().pick.model).toBeNull()
})

test('pending settings survive a remount and reload; older drafts inherit every option', () => {
  useDrafts.getState().setSettings({ approvals: false, communication: true, compactorModel: null })
  useDrafts.getState().setCreate({ memory: 'epochs', project: 'lab' })
  useDrafts.getState().setHidden(['read_file'], true)
  useDrafts.getState().setHidden(['write_file'], false)
  const settings = useDrafts.getState().settings
  useDrafts.setState(load())
  expect(useDrafts.getState().settings).toEqual(settings)
  localStorage.setItem('composer-drafts', JSON.stringify({ drafts: {}, edits: {}, pick: NO_PICK }))
  expect(load().settings).toEqual({})
})

const pending = {
  create: { memory: 'epochs' as const, project: 'lab', advisor: ['summary/model', null] as [string, string | null] },
  approvals: false,
  communication: false,
  hidden: { read_file: true, write_file: false },
  compactorModel: 'summary/model',
  compactorEffort: 'high',
  knowledgeHarvester: ['harvester/model', 'low'] as [string, string | null],
  viewBytes: 128000,
  scope: 'owner' as const,
  alias: 'research',
}
const setters = [
  ['set_approvals', 'approvals'],
  ['set_communication', 'messaging'],
  ['set_mcp_hidden', 'tools and MCP switches'],
  ['set_session_compactor_model', 'summary model'],
  ['set_session_compactor_effort', 'summary effort'],
  ['set_session_knowledge_harvester', 'knowledge harvester'],
  ['set_session_view_bytes', 'memory view size'],
  ['set_peer_scope', 'messaging scope and alias'],
] as const

function firstApi(fail?: string) {
  const calls: [string, unknown[]][] = []
  let rejected = false
  const api = new Proxy({} as Api, {
    get: (_, name: string) => async (...args: unknown[]) => {
      calls.push([name, args])
      if (name === fail && !rejected) {
        rejected = true
        throw new Error('refused')
      }
      return name === 'create_session' ? opened : undefined
    },
  })
  const send = () => sendFirst({
    api, options: null, input: 'first', machine: () => null,
    created: async () => { calls.push(['created', []]) },
    send: (id) => api.send_input(id, 'keep me', [], null, 'first'),
  })
  return { api, calls, send }
}

test('first send awaits create, then every pending setter, then send; spends settings only after application', async () => {
  const { calls, send } = firstApi()
  useDrafts.getState().setSettings(pending)
  useDraftTarget.getState().set({ engine: 'xeus', cwd: '/work' })
  await send()
  expect(calls).toEqual([
    ['create_session', [{ ...PLAIN_OPTIONS, ...pending.create, engine: 'xeus', cwd: '/work' }]],
    ['set_approvals', ['s-new', false, false]],
    ['set_communication', ['s-new', false]],
    ['set_mcp_hidden', ['s-new', ['write_file'], false]],
    ['set_mcp_hidden', ['s-new', ['read_file'], true]],
    ['set_session_compactor_model', ['s-new', 'summary/model']],
    ['set_session_compactor_effort', ['s-new', 'high']],
    ['set_session_knowledge_harvester', ['s-new', 'harvester/model', 'low']],
    ['set_session_view_bytes', ['s-new', 128000]],
    ['set_peer_scope', ['s-new', 'owner', 'research']],
    ['created', []],
    ['send_input', ['s-new', 'keep me', [], null, 'first']],
  ])
  expect(load().settings).toEqual({})
  expect(load().opening).toBeNull()
})

test.each(setters)('%s failure keeps text, attachments, settings and created chat across reload; retry never creates twice', async (method, label) => {
  const { calls, send } = firstApi(method)
  useDrafts.getState().set('', { text: 'keep me', pastes: held, unsent: 'first' })
  useDrafts.getState().setSettings(pending)
  useDraftTarget.getState().set({ engine: 'xeus', cwd: '/work' })
  await expect(send()).rejects.toThrow(`Could not apply ${label}: refused`)
  expect(calls.some(([name]) => name === 'send_input' || name === 'created')).toBe(false)
  useDrafts.setState(load())
  expect(useDrafts.getState().by['']).toEqual({ text: 'keep me', pastes: held, unsent: 'first' })
  expect(useDrafts.getState().settings).toEqual(pending)
  expect(useDrafts.getState().opening?.open.session).toBe('s-new')
  await send()
  expect(calls.filter(([name]) => name === 'create_session')).toHaveLength(1)
  expect(calls.at(-1)).toEqual(['send_input', ['s-new', 'keep me', [], null, 'first']])
  expect(load().settings).toEqual({})
})

test('a create failure retains pending settings; explicit inherit and cleared summary picks still apply', async () => {
  const { api, calls } = firstApi('create_session')
  useDrafts.getState().setSettings({ approvals: null, compactorModel: null, compactorEffort: null })
  await expect(openDraftSession(api)).rejects.toThrow('refused')
  expect(load().settings).toEqual({ approvals: null, compactorModel: null, compactorEffort: null })
  await openDraftSession(api)
  expect(calls.slice(-3)).toEqual([
    ['set_approvals', ['s-new', true, true]],
    ['set_session_compactor_model', ['s-new', null]],
    ['set_session_compactor_effort', ['s-new', null]],
  ])
})

test('openDraftSession with deferSpend keeps pick, settings, and target intact until spend() is called; spend() is idempotent', async () => {
  const { api } = firstApi()
  useDrafts.getState().setPick({ model: 'openai/gpt-5.4', effort: 'high' })
  useDrafts.getState().setSettings({ approvals: false })
  useDraftTarget.getState().set({ engine: 'xeus', cwd: '/work' })

  const r = await openDraftSession(api, null, undefined, { deferSpend: true })
  expect(r.id).toBe('s-new')
  // Still intact before spend()
  expect(useDrafts.getState().pick).toEqual({ model: 'openai/gpt-5.4', effort: 'high' })
  expect(useDrafts.getState().settings).toEqual({ approvals: false })
  expect(useDraftTarget.getState()).toMatchObject({ engine: 'xeus', cwd: '/work', reuse: null })

  r.spend()
  expect(useDrafts.getState().pick).toEqual(NO_PICK)
  expect(useDrafts.getState().settings).toEqual({})
  expect(useDraftTarget.getState()).toMatchObject({ engine: null, cwd: null, reuse: null })

  // Idempotent: a second spend() does not clobber subsequent draft state.
  useDrafts.getState().setPick({ model: 'anthropic/claude-opus-4-1', effort: 'low' })
  useDraftTarget.getState().set({ engine: 'cloud', cwd: '/next' })
  r.spend()
  expect(useDrafts.getState().pick).toEqual({ model: 'anthropic/claude-opus-4-1', effort: 'low' })
  expect(useDraftTarget.getState()).toMatchObject({ engine: 'cloud', cwd: '/next' })

  // Default call without deferSpend still spends immediately.
  await openDraftSession(api)
  expect(useDrafts.getState().pick).toEqual(NO_PICK)
  expect(useDraftTarget.getState()).toMatchObject({ engine: null, cwd: null, reuse: null })
})

test('sendFirst keeps pick, settings, and target intact while send is pending; spends on send resolve unless deferSpend is true', async () => {
  const { api } = firstApi()

  // Case 1: without deferSpend — intact while send is pending, spent once send resolves.
  useDrafts.getState().setPick({ model: 'openai/gpt-5.4', effort: 'high' })
  useDrafts.getState().setSettings({ approvals: false })
  useDraftTarget.getState().set({ engine: 'xeus', cwd: '/work' })
  let releaseSend!: () => void
  let markSendStarted!: () => void
  const sendStarted = new Promise<void>((r) => (markSendStarted = r))
  const run1 = sendFirst({
    api,
    options: null,
    input: 'i1',
    machine: () => null,
    created: async () => {},
    send: async () => {
      markSendStarted()
      await new Promise<void>((r) => (releaseSend = r))
    },
  })
  await sendStarted
  expect(useDrafts.getState().pick).toEqual({ model: 'openai/gpt-5.4', effort: 'high' })
  expect(useDrafts.getState().settings).toEqual({ approvals: false })
  expect(useDraftTarget.getState()).toMatchObject({ engine: 'xeus', cwd: '/work' })
  releaseSend()
  await run1
  expect(useDrafts.getState().pick).toEqual(NO_PICK)
  expect(useDrafts.getState().settings).toEqual({})
  expect(useDraftTarget.getState()).toMatchObject({ engine: null, cwd: null, reuse: null })

  // Case 2: with deferSpend: true — stays intact even after send resolves, until created's spend callback is called.
  useDrafts.getState().setPick({ model: 'openai/gpt-5.4', effort: 'high' })
  useDrafts.getState().setSettings({ approvals: false })
  useDraftTarget.getState().set({ engine: 'xeus', cwd: '/work' })
  let capturedSpend: (() => void) | null = null
  await sendFirst({
    api,
    options: null,
    input: 'i2',
    deferSpend: true,
    machine: () => null,
    created: async (_id, _open, _pick, spend) => {
      capturedSpend = spend
    },
    send: async () => {},
  })
  expect(useDrafts.getState().pick).toEqual({ model: 'openai/gpt-5.4', effort: 'high' })
  expect(useDrafts.getState().settings).toEqual({ approvals: false })
  expect(useDraftTarget.getState()).toMatchObject({ engine: 'xeus', cwd: '/work' })
  capturedSpend!()
  expect(useDrafts.getState().pick).toEqual(NO_PICK)
  expect(useDrafts.getState().settings).toEqual({})
  expect(useDraftTarget.getState()).toMatchObject({ engine: null, cwd: null, reuse: null })
})
