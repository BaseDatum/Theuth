import { expect, test, vi } from 'vitest'
import { notify as toast } from '@/lib/notify'
import type { Api } from '@/rpc/bindings/client'
import { NO_PICK, useDrafts } from '@/lib/drafts'
import { active } from '@/store/engine-scope'
import { useSessions } from '@/store/sessions'
import { placement, pluginInfo } from '@/components/plugins/test-fixtures'
import { EMPTY_PLUGINS, commandsOf, pluginStore, seed } from '@/lib/plugins'
import { COMMANDS, type Ctx, isCommand, run, sendRaw, slashMenu } from './slash'
import { useToolsSheet } from './tools-sheet'

vi.mock('@/lib/notify', () => ({ notify: Object.assign(vi.fn(), { error: vi.fn(), message: vi.fn() }) }))

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

test('bare /skills carries the chat; /mcp and /tools open the current chat or draft Tools sheet', async () => {
  const navigate = vi.fn().mockResolvedValue(undefined)
  const ctx = { api: {} as Api, session: 'abc', parent: null, navigate, reload: async () => {} } as unknown as Ctx
  await run('/skills', ctx)
  expect(navigate).toHaveBeenLastCalledWith({ to: '/skills', search: { session: 'abc' } })
  await run('/skills', { ...ctx, session: null })
  expect(navigate).toHaveBeenLastCalledWith({ to: '/skills', search: {} })
  navigate.mockClear()
  for (const [cmd, session] of [['/tools', 'abc'], ['/mcp', 'pane-2']] as const) {
    useToolsSheet.getState().hide()
    await run(cmd, { ...ctx, session })
    expect(useToolsSheet.getState().session).toBe(session)
  }
  expect(navigate).not.toHaveBeenCalled()
  useToolsSheet.getState().hide()
  for (const cmd of ['/mcp', '/tools']) {
    await run(cmd, { ...ctx, session: null })
    expect(useToolsSheet.getState().session).toBe('')
  }
  expect(navigate).not.toHaveBeenCalled()
  await expect(run('/tools bogus', ctx)).rejects.toThrow('usage: /tools [add <url|json> | sync]')
})

test('206: /mcp opens the sheet on MCP servers, /builtins on Builtins, /tools on the tab last shown', async () => {
  const ctx = { api: {} as Api, session: 'abc', parent: null, navigate: vi.fn(), reload: async () => {} } as unknown as Ctx
  useToolsSheet.setState({ session: null, tab: 'builtins' })
  await run('/mcp', ctx)
  expect(useToolsSheet.getState()).toMatchObject({ session: 'abc', tab: 'mcp' })
  await run('/builtins', { ...ctx, session: null })
  expect(useToolsSheet.getState()).toMatchObject({ session: '', tab: 'builtins' })
  await run('/tools', ctx)
  expect(useToolsSheet.getState()).toMatchObject({ session: 'abc', tab: 'builtins' })
  useToolsSheet.setState({ tab: 'mcp' })
  await run('/tools', ctx)
  expect(useToolsSheet.getState().tab).toBe('mcp')
  await expect(run('/builtins off', ctx)).rejects.toThrow('usage: /builtins')
  useToolsSheet.setState({ session: null, tab: 'builtins' })
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

test('157 D3: /fractal switches an open session, keeps a draft choice pending, and rejects a bad argument', async () => {
  const set_session_memory = vi.fn().mockResolvedValue({ mode: 'endless', view: [] })
  expect(isCommand('/fractal on', SKILLS)).toBe(true)
  await run('/fractal on', ctxFor({ set_session_memory }, 'abc'))
  expect(set_session_memory).toHaveBeenCalledWith('abc', 'endless')
  await run('/fractal off', ctxFor({ set_session_memory }, 'abc'))
  expect(set_session_memory).toHaveBeenLastCalledWith('abc', 'epochs')
  await expect(run('/fractal maybe', ctxFor({ set_session_memory }, 'abc'))).rejects.toThrow(/^\/fractal: expected on\|off/)
  await run('/fractal on', { ...ctxFor({ set_session_memory }), session: null })
  expect(useDrafts.getState().settings.create?.memory).toBe('endless')
  useDrafts.setState({ settings: {} })
  expect(set_session_memory).toHaveBeenCalledTimes(2)
})

test('/endless is a hidden alias of /fractal: it runs, the popup lists only /fractal, and /autocontext is no command', async () => {
  const set_session_memory = vi.fn().mockResolvedValue({ mode: 'epochs', view: [] })
  expect(isCommand('/endless off', SKILLS)).toBe(true)
  await run('/endless off', ctxFor({ set_session_memory }, 'abc'))
  expect(set_session_memory).toHaveBeenCalledWith('abc', 'epochs')
  await expect(run('/endless maybe', ctxFor({ set_session_memory }, 'abc'))).rejects.toThrow(/^\/fractal: /)
  expect(slashMenu('/', null).map((c) => c.name)).toContain('/fractal')
  expect(slashMenu('/', null).map((c) => c.name)).not.toContain('/autocontext')
  expect(slashMenu('/', null).map((c) => c.name)).not.toContain('/endless')
  expect(slashMenu('/en', null).map((c) => c.name)).not.toContain('/endless')
})

test('162 D8: /provider refresh <name> toasts the counts, bare refresh opens Providers, an engine error surfaces verbatim', async () => {
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
  expect(ctx.navigate).toHaveBeenLastCalledWith({ to: '/settings/$tab', params: { tab: 'providers' }, search: {} })
  refresh_provider_models.mockRejectedValueOnce(new Error('rwx has no key; set it first'))
  await expect(run('/provider refresh rwx', ctxFor({ refresh_provider_models }))).rejects.toThrow('rwx has no key; set it first')
  await expect(run('/provider bogus', ctxFor({}))).rejects.toThrow('usage: /provider [add <name> <url> [key] | find | limits <provider> <model> <context> [output] | refresh <name> | rm <name>]')
  expect(refresh_provider_models).toHaveBeenCalledTimes(3)
})

test('177 U6: /provider add takes an optional key (none = null, a local server), bare add opens Providers', async () => {
  const add_provider = vi.fn().mockResolvedValue({})
  await run('/provider add mlx http://127.0.0.1:8080/v1/', ctxFor({ add_provider }))
  expect(add_provider).toHaveBeenLastCalledWith('mlx', 'http://127.0.0.1:8080/v1/', null)
  await run('/provider add rwx https://rwx/v1/ sk-fake', ctxFor({ add_provider }))
  expect(add_provider).toHaveBeenLastCalledWith('rwx', 'https://rwx/v1/', 'sk-fake')
  const ctx = ctxFor({ add_provider })
  await run('/provider add mlx', ctx)
  expect(ctx.navigate).toHaveBeenLastCalledWith({ to: '/settings/$tab', params: { tab: 'providers' }, search: {} })
  // The engine's refusal of a keyless cloud URL surfaces verbatim.
  add_provider.mockRejectedValueOnce(new Error('api key is empty'))
  await expect(run('/provider add rwx https://rwx/v1/', ctxFor({ add_provider }))).rejects.toThrow('api key is empty')
  expect(add_provider).toHaveBeenCalledTimes(3)
})

test('188: /peers opens this chat’s Messaging sheet; without a chat it fails', async () => {
  const ctx = ctxFor({})
  await run('/peers', ctx)
  expect(ctx.navigate).toHaveBeenLastCalledWith({ to: '/c/$session', params: { session: 'abc' }, search: { messaging: '1' } })
  await expect(run('/peers', { ...ctxFor({}), session: null })).rejects.toThrow('no session')
})

test('177 U6: /provider find probes and lists each server in one toast', async () => {
  const local_servers_probe = vi.fn().mockResolvedValue([
    { runtime: 'ollama', base_url: 'http://127.0.0.1:11434/v1/', reachable: true, models: 3, added_as: null, error: null },
    { runtime: 'omlx', base_url: 'http://127.0.0.1:8080/v1/', reachable: true, models: 2, added_as: 'mlx', error: null },
    { runtime: 'lmstudio', base_url: 'http://127.0.0.1:1234/v1/', reachable: false, models: 0, added_as: null, error: 'connection refused' },
  ])
  await run('/provider find', ctxFor({ local_servers_probe }))
  expect(toast.message).toHaveBeenLastCalledWith('Local servers', {
    description: 'Ollama http://127.0.0.1:11434/v1/ · 3 models\nomlx http://127.0.0.1:8080/v1/ · 2 models · added as mlx\nLM Studio http://127.0.0.1:1234/v1/ · not running · connection refused',
  })
  local_servers_probe.mockResolvedValueOnce([])
  await run('/provider find', ctxFor({ local_servers_probe }))
  expect(toast.message).toHaveBeenLastCalledWith('Local servers', { description: 'nothing answered on this node' })
})

test('177 R1-4: /provider add, rm, refresh and limits refetch the providers and models queries', async () => {
  const spy = vi.spyOn(active().qc, 'invalidateQueries')
  const provider = { name: 'mlx', base_url: 'http://127.0.0.1:8080/v1/', models: 2 }
  const api = {
    add_provider: vi.fn().mockResolvedValue(provider),
    remove_provider: vi.fn().mockResolvedValue(null),
    refresh_provider_models: vi.fn().mockResolvedValue({ provider, added: [], removed: [] }),
    set_model_limits: vi.fn().mockResolvedValue({ id: 'LFM3', name: 'LFM3', context: 32_768, output: 0, source: 'user', assumed_context: null }),
  }
  for (const line of ['/provider add mlx http://127.0.0.1:8080/v1/', '/provider rm mlx', '/provider refresh mlx', '/provider limits mlx LFM3 32768 0']) {
    spy.mockClear()
    await run(line, ctxFor(api))
    expect(spy.mock.calls.map((c) => c[0]?.queryKey)).toEqual([['providers'], ['models']])
  }
  spy.mockRestore()
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

test('draft slash settings stay pending, including explicit off; none calls a session or global setter', async () => {
  useDrafts.setState({ settings: {} })
  const api = { set_approvals: vi.fn(), set_communication: vi.fn(), set_session_project: vi.fn(), set_session_memory: vi.fn() }
  const ctx = { ...ctxFor(api), session: null }
  await run('/approvals off', ctx)
  await run('/communication on', ctx)
  await run('/fractal off', ctx)
  await run('/project lab', ctx)
  expect(useDrafts.getState().settings).toEqual({ approvals: false, communication: true, create: { memory: 'epochs', project: 'lab' } })
  await run('/project off', ctx)
  expect(useDrafts.getState().settings.create?.project).toBeNull()
  for (const call of Object.values(api)) expect(call).not.toHaveBeenCalled()
  useDrafts.setState({ settings: {} })
})

test('/login on a local engine calls login once and leaves the browser to the engine; the URL is a fallback toast', async () => {
  const open = vi.fn()
  vi.stubGlobal('window', { open })
  const login = vi.fn().mockResolvedValue('https://auth.example/authorize')
  await run('/login anthropic', ctxFor({ login }))
  expect(login).toHaveBeenCalledTimes(1)
  expect(open).not.toHaveBeenCalled()
  expect(toast.message).toHaveBeenLastCalledWith("Browser didn't open?", { description: 'https://auth.example/authorize' })
  vi.unstubAllGlobals()
})

// 216 §9.1: a running plugin's commands, one bare (`/turns`) and one named (`/turns:reset`).
const TURNS = pluginInfo('turns', [
  placement('turns-cmd', { kind: 'command', format: null, command: { name: 'turns', slash: '/turns', description: 'Turns today', args: null } }),
  placement('reset', { kind: 'command', format: null, command: { name: 'reset', slash: '/turns:reset', description: 'Reset the count', args: '[day]' } }),
])
const PLUGIN_COMMANDS = commandsOf([TURNS])

test('plugin commands close the menu after the built-ins and the harness, labelled; COMMANDS stay as they were', () => {
  const before = COMMANDS.map((c) => c.name)
  const all = slashMenu('/', null, PLUGIN_COMMANDS)
  expect(all.slice(0, COMMANDS.length).map((c) => c.name)).toEqual(before)
  expect(all.slice(COMMANDS.length).map((c) => [c.name, c.hint, c.source])).toEqual([
    ['/turns', '', 'plugin · turns'],
    ['/turns:reset', '[day]', 'plugin · turns'],
  ])
  const harness = { agent: 'omp', commands: [{ name: 'turns', description: "omp's own" }], skills: [] }
  // A harness command of the same name keeps it; the plugin's other command still lists.
  expect(slashMenu('/turns', harness, PLUGIN_COMMANDS).map((c) => [c.name, c.source])).toEqual([
    ['/turns', 'omp'],
    ['/turns:reset', 'plugin · turns'],
  ])
  expect(COMMANDS.map((c) => c.name)).toEqual(before)
})

test('a plugin command is a command with or without arguments; without the plugin it is text', () => {
  expect(isCommand('/turns:reset today', [], PLUGIN_COMMANDS)).toBe(true)
  expect(isCommand('/turns', [], PLUGIN_COMMANDS)).toBe(true)
  expect(isCommand('/turns:reset today', [])).toBe(false)
  expect(isCommand('/turns:nope', [], PLUGIN_COMMANDS)).toBe(false)
})

test('a hidden plugin command leaves the menu but still runs when typed', () => {
  const hidden = commandsOf([pluginInfo('turns', [placement('reset', { kind: 'command', hidden: true, format: null, command: { name: 'reset', slash: '/turns:reset', description: 'Reset', args: null } })])])
  expect(slashMenu('/turns', null, hidden)).toEqual([])
  expect(isCommand('/turns:reset', [], hidden)).toBe(true)
})

test('running a plugin command calls plugin_command; a ui reply becomes the card, a text reply a note', async () => {
  const store = pluginStore(active())
  store.setState(seed(EMPTY_PLUGINS, 0, [TURNS], []))
  const ui = { format: 'markdown' as const, source: '**0**', text: '0' }
  const plugin_command = vi.fn(async () => ({ text: null, ui }))
  await run('/turns:reset today', ctxFor({ plugin_command }))
  expect(plugin_command).toHaveBeenCalledExactlyOnceWith('turns', 'reset', 'today', 'abc')
  expect(store.getState().card).toEqual({ plugin: 'turns', slash: '/turns:reset', session: 'abc', ui })
  plugin_command.mockResolvedValueOnce({ text: 'Turns today: 3', ui: null } as never)
  await run('/turns', ctxFor({ plugin_command }, 'abc'))
  expect(plugin_command).toHaveBeenLastCalledWith('turns', 'turns', '', 'abc')
  expect(toast).toHaveBeenLastCalledWith('Turns today: 3')
  store.setState(EMPTY_PLUGINS)
})
