import { expect, test, vi } from 'vitest'
import { toast } from 'sonner'
import type { Api } from '@/rpc/api'
import { NO_PICK, useDrafts } from '@/lib/drafts'
import { useSessions } from '@/store/sessions'
import { type Ctx, isCommand, run, sendRaw, slashMenu } from './slash'

vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn(), message: vi.fn() }) }))

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

test('148 R1: /model and /effort with no session set the draft pick only — never set_model/set_effort with a null session', async () => {
  useDrafts.getState().resetPick()
  const set_model = vi.fn()
  const set_effort = vi.fn()
  const ctx: Ctx = { ...ctxFor({ set_model, set_effort }), session: null }
  await run('/model openai/gpt-5.4', ctx)
  await run('/effort high', ctx)
  expect(set_model).not.toHaveBeenCalled()
  expect(set_effort).not.toHaveBeenCalled()
  expect(useDrafts.getState().pick).toEqual({ model: 'openai/gpt-5.4', effort: 'high' })
  await run('/effort clear', ctx)
  expect(useDrafts.getState().pick.effort).toBeNull()
  useDrafts.getState().resetPick()
  expect(useDrafts.getState().pick).toEqual(NO_PICK)
})

test('148 R1: in a session /model and /effort switch that session (manual), leaving the draft pick alone', async () => {
  const set_model = vi.fn().mockResolvedValue({ id: 'openai/gpt-5.4' })
  const set_effort = vi.fn().mockResolvedValue(null)
  await run('/model openai/gpt-5.4', ctxFor({ set_model, set_effort }, 'abc'))
  await run('/effort clear', ctxFor({ set_model, set_effort }, 'abc'))
  expect(set_model).toHaveBeenCalledWith('abc', 'openai/gpt-5.4', true)
  expect(set_effort).toHaveBeenCalledWith('abc', null, true)
  expect(useDrafts.getState().pick).toEqual(NO_PICK)
})

test('157 D3: /endless on|off switches the session through set_session_memory; a draft and a bad argument are refused', async () => {
  const set_session_memory = vi.fn().mockResolvedValue({ mode: 'endless', view: [] })
  expect(isCommand('/endless on', SKILLS)).toBe(true)
  await run('/endless on', ctxFor({ set_session_memory }, 'abc'))
  expect(set_session_memory).toHaveBeenCalledWith('abc', 'endless')
  await run('/endless off', ctxFor({ set_session_memory }, 'abc'))
  expect(set_session_memory).toHaveBeenLastCalledWith('abc', 'epochs')
  await expect(run('/endless maybe', ctxFor({ set_session_memory }, 'abc'))).rejects.toThrow(/on\|off/)
  await expect(run('/endless on', { ...ctxFor({ set_session_memory }), session: null })).rejects.toThrow(/no session yet/)
  expect(set_session_memory).toHaveBeenCalledTimes(2)
})

test('162 D8: /provider refresh <name> toasts the counts, bare refresh opens Connections, an engine error surfaces verbatim', async () => {
  const provider = { name: 'rwx', base_url: 'https://rwx/v1/', models: 14, builtin: false, configured: true, refreshable: true }
  const refresh_provider_models = vi.fn().mockResolvedValue({ provider, added: ['a', 'b'], removed: ['c'] })
  await run('/provider refresh rwx', ctxFor({ refresh_provider_models }))
  expect(refresh_provider_models).toHaveBeenCalledWith('rwx')
  expect(toast).toHaveBeenLastCalledWith('rwx: 14 models (+2, −1)')
  refresh_provider_models.mockResolvedValueOnce({ provider, added: [], removed: [] })
  await run('/provider refresh rwx', ctxFor({ refresh_provider_models }))
  expect(toast).toHaveBeenLastCalledWith('rwx: 14 models, no change')
  const ctx = ctxFor({ refresh_provider_models })
  await run('/provider refresh', ctx)
  expect(ctx.navigate).toHaveBeenLastCalledWith({ to: '/settings/$tab', params: { tab: 'connections' }, search: { session: 'abc' } })
  refresh_provider_models.mockRejectedValueOnce(new Error('rwx has no key; set it first'))
  await expect(run('/provider refresh rwx', ctxFor({ refresh_provider_models }))).rejects.toThrow('rwx has no key; set it first')
  await expect(run('/provider bogus', ctxFor({}))).rejects.toThrow('usage: /provider [add <name> <url> [key] | find | limits <provider> <model> <context> [output] | refresh <name> | rm <name>]')
  expect(refresh_provider_models).toHaveBeenCalledTimes(3)
})

test('177 U6: /provider add takes an optional key (none = null, a local server), bare add opens Connections', async () => {
  const add_provider = vi.fn().mockResolvedValue({})
  await run('/provider add mlx http://127.0.0.1:8080/v1/', ctxFor({ add_provider }))
  expect(add_provider).toHaveBeenLastCalledWith('mlx', 'http://127.0.0.1:8080/v1/', null)
  await run('/provider add rwx https://rwx/v1/ sk-fake', ctxFor({ add_provider }))
  expect(add_provider).toHaveBeenLastCalledWith('rwx', 'https://rwx/v1/', 'sk-fake')
  const ctx = ctxFor({ add_provider })
  await run('/provider add mlx', ctx)
  expect(ctx.navigate).toHaveBeenLastCalledWith({ to: '/settings/$tab', params: { tab: 'connections' }, search: { session: 'abc' } })
  // The engine's refusal of a keyless cloud URL surfaces verbatim.
  add_provider.mockRejectedValueOnce(new Error('api key is empty'))
  await expect(run('/provider add rwx https://rwx/v1/', ctxFor({ add_provider }))).rejects.toThrow('api key is empty')
  expect(add_provider).toHaveBeenCalledTimes(3)
})

test('177 U6: /provider find probes and lists each server in one toast', async () => {
  const local_servers_probe = vi.fn().mockResolvedValue([
    { runtime: 'ollama', base_url: 'http://127.0.0.1:11434/v1/', reachable: true, models: 3, added_as: null, error: null },
    { runtime: 'omlx', base_url: 'http://127.0.0.1:8080/v1/', reachable: true, models: 2, added_as: 'mlx', error: null },
    { runtime: 'lmstudio', base_url: 'http://127.0.0.1:1234/v1/', reachable: false, models: 0, added_as: null, error: 'connection refused' },
  ])
  await run('/provider find', ctxFor({ local_servers_probe }))
  expect(toast.message).toHaveBeenLastCalledWith('Local servers', {
    description: 'Ollama http://127.0.0.1:11434/v1/ · 3 models\nomlx http://127.0.0.1:8080/v1/ · 2 models · added as mlx\nLM Studio http://127.0.0.1:1234/v1/ · not running',
  })
})

test('177 U6: /provider limits <provider> <model> <context> [output] — 0 = unknown, an omitted output keeps the stored one, bad numbers are refused', async () => {
  const set_model_limits = vi.fn().mockImplementation(async (_p: string, id: string, context: number | null, output: number | null) => ({ id, name: id, context: context ?? 0, output: output ?? 0, source: 'user', assumed_context: null }))
  const provider_models = vi.fn().mockResolvedValue([{ id: 'LFM3', name: 'LFM3', context: 0, output: 1024, source: 'server', assumed_context: 16_384 }])
  await run('/provider limits mlx LFM3 32768 2048', ctxFor({ set_model_limits, provider_models }))
  expect(set_model_limits).toHaveBeenLastCalledWith('mlx', 'LFM3', 32_768, 2_048)
  expect(provider_models).not.toHaveBeenCalled()
  expect(toast).toHaveBeenLastCalledWith('mlx/LFM3: context 32,768 · output 2,048')
  await run('/provider limits mlx LFM3 32768', ctxFor({ set_model_limits, provider_models }))
  expect(set_model_limits).toHaveBeenLastCalledWith('mlx', 'LFM3', 32_768, 1_024)
  await run('/provider limits mlx LFM3 0 0', ctxFor({ set_model_limits, provider_models }))
  expect(set_model_limits).toHaveBeenLastCalledWith('mlx', 'LFM3', null, null)
  expect(toast).toHaveBeenLastCalledWith('mlx/LFM3: context unknown · output unknown')
  for (const bad of ['/provider limits mlx LFM3', '/provider limits mlx', '/provider limits mlx LFM3 big', '/provider limits mlx LFM3 4096 -1', '/provider limits mlx LFM3 1.5'])
    await expect(run(bad, ctxFor({ set_model_limits, provider_models }))).rejects.toThrow('usage: /provider limits <provider> <model> <context> [output]')
  expect(set_model_limits).toHaveBeenCalledTimes(3)
})
