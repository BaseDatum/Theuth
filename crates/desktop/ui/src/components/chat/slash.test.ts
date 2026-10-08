import { expect, test, vi } from 'vitest'
import { toast } from 'sonner'
import type { Api } from '@/rpc/api'
import { useSessions } from '@/store/sessions'
import { type Ctx, isCommand, run, sendRaw, slashMenu } from './slash'

vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn() }) }))

/** One macrotask hop: every microtask queued so far (promise continuations) has run. */
const flush = () => new Promise<void>((r) => setImmediate(r))
const ctxFor = (api: Partial<Api>, session = 'abc'): Ctx =>
  ({ api: api as Api, session, parent: null, navigate: vi.fn(), reload: async () => {} }) as Ctx
const activity = (session: string) => useSessions.getState().sessions[session]?.activity

const SKILLS = ['caveman', 'ponytail']

test('built-ins and known skills are commands, with or without arguments', () => {
  expect(isCommand('/model', SKILLS)).toBe(true)
  expect(isCommand('/compact summary', SKILLS)).toBe(true)
  expect(isCommand('/caveman lite', SKILLS)).toBe(true)
  expect(isCommand('/ponytail', [])).toBe(false)
})

test('an unknown word, a path or a bare slash is message text', () => {
  expect(isCommand('/foo bar', SKILLS)).toBe(false)
  expect(isCommand('/Users/x/y is broken, look at it', SKILLS)).toBe(false)
  expect(isCommand('/etc/hosts has a bad entry', SKILLS)).toBe(false)
  expect(isCommand('/mod', SKILLS)).toBe(false) // a prefix is not the command
  expect(isCommand('/', SKILLS)).toBe(false)
  expect(isCommand('hello /model', SKILLS)).toBe(false)
})

test('skill index not loaded: a plain word still runs, a path-like word is text', () => {
  expect(isCommand('/foo bar', null)).toBe(true)
  expect(isCommand('/Users/x/y is broken', null)).toBe(false)
  expect(isCommand('/foo.txt is broken', null)).toBe(false)
  expect(isCommand('/', null)).toBe(false)
})

test('bare /skills and /mcp open their pages, carrying the chat they came from', async () => {
  const navigate = vi.fn().mockResolvedValue(undefined)
  const ctx = { api: {} as Api, session: 'abc', parent: null, navigate, reload: async () => {} } as unknown as Ctx
  await run('/skills', ctx)
  expect(navigate).toHaveBeenLastCalledWith({ to: '/skills', search: { session: 'abc' } })
  await run('/mcp', ctx)
  expect(navigate).toHaveBeenLastCalledWith({ to: '/tools', search: { session: 'abc' } })
  await run('/skills', { ...ctx, session: null })
  expect(navigate).toHaveBeenLastCalledWith({ to: '/skills', search: {} })
})

// 87 COMPACT-NONBLOCK: `/compact` returns once the RPC is issued, so `submit` releases `busy` and the composer
// stays usable (the session shows `compacting`, a send steers/queues) for the minutes a compaction may take.
test('/compact snap returns before the RPC settles, flips compacting, toasts the epoch on success', async () => {
  let resolve!: (epoch: number) => void
  const compact = vi.fn(() => new Promise<number>((r) => (resolve = r)))
  const S = 'snap-sess'
  let returned = false
  void run('/compact snap', ctxFor({ compact }, S)).then(() => (returned = true))
  await flush()
  expect(returned).toBe(true)
  expect(compact).toHaveBeenCalledTimes(1)
  expect(compact).toHaveBeenCalledWith(S, 'snap')
  expect(activity(S)).toBe('compacting')
  resolve(3)
  await flush()
  expect(toast).toHaveBeenLastCalledWith(expect.stringMatching(/epoch 3.*snap/))
  expect(activity(S)).toBe('compacting') // the engine's `Compacted` resets it, not the reply
})

test('/compact refused: toast.error with the message, activity settles back to idle', async () => {
  const compact = vi.fn().mockRejectedValue(new Error('nothing to compact'))
  const S = 'refused-sess'
  await run('/compact', ctxFor({ compact }, S))
  expect(compact).toHaveBeenCalledWith(S, null)
  await flush()
  expect(toast.error).toHaveBeenCalledWith('nothing to compact')
  expect(activity(S)).toBe('idle')
})

test('/clear is fire-and-forget too and never toasts on success', async () => {
  vi.mocked(toast).mockClear()
  const compact = vi.fn().mockResolvedValue(4)
  const S = 'clear-sess'
  let returned = false
  void run('/clear', ctxFor({ compact }, S)).then(() => (returned = true))
  await flush()
  expect(returned).toBe(true)
  expect(compact).toHaveBeenCalledWith(S, 'clear')
  expect(toast).not.toHaveBeenCalled()
})

test('other commands still await their RPC (rule 4): /undo stays pending while api.undo does', async () => {
  const undo = vi.fn(() => new Promise<string>(() => {}))
  let returned = false
  void run('/undo', ctxFor({ undo })).then(() => (returned = true))
  await flush()
  expect(undo).toHaveBeenCalledWith('abc')
  expect(returned).toBe(false)
})

// 141 D15 (a): a harness skill/command row sends its text verbatim — never `/skill:` parking, never a theuth built-in.
test('sendRaw hands the line to the session as typed: send_input when idle, steer (queued) mid-turn', async () => {
  const send_input = vi.fn().mockResolvedValue('idle-sess')
  const steer = vi.fn().mockResolvedValue('in')
  const api = { send_input, steer } as unknown as Api
  await sendRaw(api, 'idle-sess', '/skill:ponytail')
  expect(send_input).toHaveBeenCalledWith('idle-sess', '/skill:ponytail', [], null, expect.any(String))
  expect(steer).not.toHaveBeenCalled()
  // Shown at once, as our own input.
  expect(useSessions.getState().sessions['idle-sess']?.entries.at(-1)).toMatchObject({ kind: 'user', text: '/skill:ponytail' })
  useSessions.getState().applyEvents([{ type: 'TurnStarted', session: 'busy-sess', turn: 't', model: 'm', text: 'go', turn_seq: 1 }])
  await sendRaw(api, 'busy-sess', '/compact')
  expect(steer).toHaveBeenCalledWith('busy-sess', '/compact', expect.any(String), [])
  expect(useSessions.getState().sessions['busy-sess']?.queued.map((q) => q.text)).toEqual(['/compact'])
})

// 141 D15 (b): the ACP session's menu merges the harness's commands under their source; typed `/skill:<name>` stays theuth's.
test('slashMenu lists harness commands beside theuth’s with source labels; a shared skill name shows theuth’s row first and the harness’s as raw', () => {
  const commands = [
    { name: 'skill:ponytail', description: 'lazy mode' },
    { name: 'skill:omp-only', description: 'only the harness has it' },
    { name: 'compact', description: 'omp compaction', hint: '[focus]' },
  ]
  // No harness: the built-ins, unlabelled.
  expect(slashMenu('/mo', null).map((c) => [c.name, c.source])).toEqual([['/model', undefined]])
  const harness = { agent: 'omp', commands, skills: ['ponytail', 'caveman'] }
  const all = slashMenu('/', harness)
  expect(all.filter((c) => c.source === 'theuth').length).toBeGreaterThan(3)
  expect(all.filter((c) => c.raw).map((c) => [c.name, c.source, c.hint])).toEqual([
    ['/skill:ponytail', 'omp', ''],
    ['/skill:omp-only', 'omp', ''],
    ['/compact', 'omp', '[focus]'],
  ])
  // Both have `ponytail`: theuth's row first (typed ⏎ parks it, says so), then omp's raw row; `omp-only` has one row.
  const pony = slashMenu('/skill:pony', harness)
  expect(pony.map((c) => [c.name, c.source, c.raw ?? false])).toEqual([
    ['/skill:ponytail', 'theuth', false],
    ['/skill:ponytail', 'omp', true],
  ])
  expect(pony[0]?.desc).toMatch(/park theuth's ponytail/)
  expect(slashMenu('/skill:omp', harness).map((c) => [c.source, c.raw])).toEqual([['omp', true]])
  // theuth's built-in `/compact` and omp's `compact` both match: theuth's first, omp's raw.
  expect(slashMenu('/compact', harness).map((c) => [c.source, c.raw ?? false])).toEqual([
    ['theuth', false],
    ['omp', true],
  ])
})
