import { beforeEach, expect, test, vi } from 'vitest'
import type { Api } from '@/rpc/api'
import { NO_PICK, openDraftSession, sendFirstElsewhere, useDraftTarget, useDrafts } from './drafts'
import { YOUNG_MS, onNoSuchSession, useRemoteStart } from './remote-start'

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

/** The new-chat composer's path for a chat on xeus: the draft is marked sending, `create_session` runs there, then the
 * first message goes through `sendFirstElsewhere`; `send` is what the engine answers. Returns the routes visited. */
async function firstMessage(send: () => Promise<unknown>, created: string[] = []): Promise<{ visited: string[]; error: unknown; id: string }> {
  useDrafts.getState().setText('', 'hello xeus')
  useDrafts.getState().sending('', 'input-1')
  useDraftTarget.getState().set({ engine: 'xeus-id', cwd: '/home/sky/src' })
  const api = { create_session: async () => (created.push('s1'), { session: 's1' }) } as unknown as Api
  const target = { engine: 'xeus-id', cwd: '/home/sky/src' }
  const { id } = await openDraftSession(api)
  const visited: string[] = []
  let error: unknown = null
  await sendFirstElsewhere({
    session: id,
    machine: 'xeus',
    target,
    open: async () => visited.push(`/c/${id}`),
    back: async () => visited.push('/new'),
    send,
  }).catch((e: unknown) => {
    error = e
  })
  return { visited, error, id }
}

test('183 §5.6: the first message to a chat whose row has not reached this engine fails: back on /new with the text and the machine, never home', async () => {
  const { visited, error, id } = await firstMessage(() => Promise.reject(new Error('s1: no such session')))
  expect(visited).toEqual([`/c/${id}`, '/new'])
  expect(visited).not.toContain('/')
  expect(useDrafts.getState().by['']).toMatchObject({ text: 'hello xeus', unsent: 'input-1' })
  // A retry from the box goes to the same machine and folder.
  expect(useDraftTarget.getState()).toMatchObject({ engine: 'xeus-id', cwd: '/home/sky/src', reuse: null })
  expect((error as Error).message).toContain('xeus')
  expect(useRemoteStart.getState().by[id]).toBeUndefined()
})

test('addenda 14:45: a first message that did not go keeps its empty chat for the retry, which sends into it instead of creating another', async () => {
  const created: string[] = []
  const { error } = await firstMessage(() => Promise.reject(new Error('send timed out')), created)
  expect(error).not.toBeNull()
  expect(useDraftTarget.getState().reuse).toEqual({ session: 's1', engine: 'xeus-id', cwd: '/home/sky/src' })
  // Send again: the same chat, nothing created; the draft still carries the client input id the engine dedupes.
  const sends: string[] = []
  const again = await firstMessage(async () => sends.push('s1'), created)
  expect(again).toMatchObject({ id: 's1', error: null })
  expect(created).toEqual(['s1'])
  expect(useDraftTarget.getState().reuse).toBeNull()
  // Another machine (or folder) never reuses it.
  useDraftTarget.getState().set({ reuse: { session: 's1', engine: 'xeus-id', cwd: null } })
  useDraftTarget.getState().set({ engine: 'cloud-id', cwd: null })
  await openDraftSession({ create_session: async () => (created.push('s2'), { session: 's2' }) } as unknown as Api)
  expect(created).toEqual(['s1', 's2'])
})

test('183 §5.6: a slow machine that takes the message: the chat opened before the reply, starting until its first event', async () => {
  let release: () => void = () => {}
  const sent = new Promise<void>((r) => (release = r))
  const run = firstMessage(() => sent)
  await vi.waitFor(() => expect(useRemoteStart.getState().by.s1).toMatchObject({ machine: 'xeus', heard: false }))
  release()
  const { visited, error } = await run
  expect(error).toBeNull()
  expect(visited).toEqual(['/c/s1'])
  useRemoteStart.getState().heard('s1')
  expect(useRemoteStart.getState().by.s1).toMatchObject({ heard: true, unreachable: false })
})

test('183 §5.6: "no such session" on the route: a chat just created elsewhere retries with backoff for YOUNG_MS, then is unreachable; any other id goes home', () => {
  const t0 = 1_000_000
  useRemoteStart.getState().begin('young', 'xeus', t0)
  const delays = [0, 1, 2, 3, 4, 5, 6].map((tries) => onNoSuchSession('young', tries, t0 + 1_000))
  expect(delays).toEqual([500, 1000, 2000, 4000, 8000, 8000, 8000].map((retry) => ({ retry })))
  expect(onNoSuchSession('young', 9, t0 + YOUNG_MS)).toBe('unreachable')
  expect(onNoSuchSession('deleted-chat', 0, t0)).toBe('home')
  // The unreachable card goes as soon as the machine is heard from.
  useRemoteStart.getState().unreachable('young')
  useRemoteStart.getState().heard('young')
  expect(useRemoteStart.getState().by.young?.unreachable).toBe(false)
})
