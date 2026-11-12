import { beforeEach, expect, test, vi } from 'vitest'
import type { Api } from '@/rpc/bindings/client'
import { NO_PICK, openDraftSession, sendFirst, useDraftTarget, useDrafts } from './drafts'
import { YOUNG_MS, reportLoadError, useRemoteStart } from './remote-start'

const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => mem.get(k) ?? null,
  setItem: (k: string, v: string) => void mem.set(k, v),
  clear: () => mem.clear(),
})

beforeEach(() => {
  localStorage.clear()
  useDrafts.setState({ by: {}, edits: {}, pick: NO_PICK })
  useRemoteStart.setState({ by: {} })
  useDraftTarget.getState().set({ engine: null, cwd: null, reuse: null })
})

const XEUS = { engine: 'xeus-id', cwd: '/home/sky/src' }

/**
 * The New chat composer's first send (`Composer` → `sendFirst`) of "hello xeus" as client input `input`, with the draft
 * marked sending as the composer does: `create_session` answers `s<n>`, `send` is what the engine answers, `open` /
 * `back` record the routes (the composer navigates there), `live` says whether the chat's follow stream comes up.
 */
async function newChat(o: { engine?: string | null; send: (id: string) => Promise<unknown>; input?: string; created?: string[]; live?: boolean }) {
  const input = o.input ?? 'input-1'
  const created = o.created ?? []
  useDrafts.getState().setText('', 'hello xeus')
  useDrafts.getState().sending('', input)
  useDraftTarget.getState().set(o.engine === null ? { engine: null, cwd: null } : XEUS)
  const api = { create_session: async () => (created.push(`s${created.length + 1}`), { session: `s${created.length}` }), new_session: async () => 'local' } as unknown as Api
  const visited: string[] = []
  const sent: string[] = []
  let error: unknown = null
  await sendFirst({
    api,
    options: null,
    input,
    machine: (engine) => (engine === XEUS.engine ? 'xeus' : null),
    created: async (id) => void visited.push(`seeded ${id}`),
    send: async (id) => {
      sent.push(id)
      return o.send(id)
    },
    open: async (id) => {
      visited.push(`/c/${id}`)
      if (o.live !== false) useRemoteStart.getState().live(id)
    },
    back: async () => void visited.push('/new'),
  }).catch((e: unknown) => {
    error = e
  })
  return { visited, sent, error }
}

test('183 §5.6: a first message refused while the row is not here: back on /new with the text and the machine, never home', async () => {
  const { visited, error } = await newChat({ send: () => Promise.reject(new Error('s1: no such session')) })
  expect(visited).toEqual(['seeded s1', '/c/s1', '/new'])
  expect(visited).not.toContain('/')
  expect(useDrafts.getState().by['']).toMatchObject({ text: 'hello xeus', unsent: 'input-1' })
  expect(useDraftTarget.getState()).toMatchObject({ ...XEUS, reuse: null })
  expect((error as Error).message).toContain('xeus')
  expect(useRemoteStart.getState().by.s1).toBeUndefined()
})

test('183 §5.6: a create that fails on the machine stays on the draft and names the machine; this machine opens after the send', async () => {
  useDrafts.getState().setText('', 'hello xeus')
  useDraftTarget.getState().set(XEUS)
  const api = { create_session: () => Promise.reject(new Error('owner 2325 unreachable: not connected')) } as unknown as Api
  const visit = vi.fn(async () => undefined)
  const e = await sendFirst({ api, options: null, input: 'i', machine: () => 'xeus', created: visit, send: visit, open: visit, back: visit }).catch((x: unknown) => x)
  expect((e as Error).message).toBe('Could not start the chat on xeus: owner 2325 unreachable: not connected')
  expect(visit).not.toHaveBeenCalled()
  expect(useDrafts.getState().by['']?.text).toBe('hello xeus')
  // This machine: no open/back here; the composer navigates once the send is taken.
  const { visited, sent } = await newChat({ engine: null, send: async () => undefined })
  expect(visited).toEqual(['seeded local'])
  expect(sent).toEqual(['local'])
})

test('183 §5.6: the chat opens before the send, which waits for its follow stream (no replay), at most 2 s', async () => {
  vi.useFakeTimers()
  try {
    const sends: string[] = []
    const send = async (id: string) => void sends.push(id)
    const run = newChat({ send, live: false })
    await vi.advanceTimersByTimeAsync(0)
    expect(useRemoteStart.getState().by.s1).toMatchObject({ machine: 'xeus', live: false })
    expect(sends).toEqual([])
    useRemoteStart.getState().live('s1')
    await vi.advanceTimersByTimeAsync(0)
    expect(sends).toEqual(['s1'])
    const { visited, error } = await run
    expect(error).toBeNull()
    expect(visited).toEqual(['seeded s1', '/c/s1'])
    // No stream comes up: the send still goes after the bound.
    const slow = newChat({ send, live: false, created: ['s1'] })
    await vi.advanceTimersByTimeAsync(1_999)
    expect(sends).toEqual(['s1'])
    await vi.advanceTimersByTimeAsync(1)
    await slow
    expect(sends).toEqual(['s1', 's2'])
  } finally {
    vi.useRealTimers()
  }
})

test('addenda 14:45: a first message that did not go keeps its empty chat; resending it goes into that chat, another message does not', async () => {
  const created: string[] = []
  const first = await newChat({ send: () => Promise.reject(new Error('send timed out')), created })
  expect(first.error).not.toBeNull()
  expect(useDraftTarget.getState().reuse).toEqual({ session: 's1', ...XEUS, input: 'input-1' })
  // Send again (same client input id, which the engine dedupes): the same chat, nothing created.
  const again = await newChat({ send: async () => undefined, created })
  expect(again.sent).toEqual(['s1'])
  expect(created).toEqual(['s1'])
  expect(useDraftTarget.getState().reuse).toBeNull()
  // A different message to the same machine and folder never lands in it.
  useDraftTarget.getState().set({ reuse: { session: 's1', ...XEUS, input: 'input-1' } })
  const other = await newChat({ send: async () => undefined, created, input: 'input-2' })
  expect(other.sent).toEqual(['s2'])
  // Nor does another machine.
  useDraftTarget.getState().set({ reuse: { session: 's1', ...XEUS, input: 'input-1' }, engine: 'cloud-id', cwd: null })
  await openDraftSession({ create_session: async () => (created.push('s3'), { session: 's3' }) } as unknown as Api, null, 'input-1')
  expect(created).toEqual(['s1', 's2', 's3'])
})

test('183 §5.6: the chat route on "no such session": a chat just created elsewhere retries with backoff for YOUNG_MS, then the card — never home', () => {
  const io = () => ({ home: vi.fn(), toast: vi.fn(), retry: vi.fn() })
  const nss = new Error('young: no such session')
  const t0 = 1_000_000
  useRemoteStart.getState().begin('young', 'xeus', t0)
  const tries = { current: 0 }
  const a = io()
  for (let i = 0; i < 7; i++) reportLoadError(nss, 'young', tries, a, t0 + 1_000)
  expect(a.retry.mock.calls.map(([ms]) => ms)).toEqual([500, 1000, 2000, 4000, 8000, 8000, 8000])
  const b = io()
  reportLoadError(nss, 'young', tries, b, t0 + YOUNG_MS)
  expect(useRemoteStart.getState().by.young?.unreachable).toBe(true)
  for (const x of [a, b]) expect(x.home).not.toHaveBeenCalled()
  // Any other id goes home silently (a deleted chat, a stale link); other errors toast.
  const c = io()
  reportLoadError(new Error('deleted: no such session'), 'deleted', { current: 0 }, c)
  reportLoadError(new Error('boom'), 'young', { current: 0 }, c)
  expect(c.home).toHaveBeenCalledTimes(1)
  expect(c.toast).toHaveBeenCalledWith('boom')
  // The machine is heard from: the entry (and the card) goes.
  useRemoteStart.getState().forget('young')
  expect(useRemoteStart.getState().by.young).toBeUndefined()
})
