import { expect, test } from 'vitest'
import { STUCK_MS, type StartupInput, cleanLog, lastLogLine, startupView } from './startup-progress'

const BASE: StartupInput = { started: false, owned: null, state: 'connecting', info: false, log: '', waited: 0 }
const LOG = 'Last login: Tue Nov 17 on ttys004\r\n\u001b]0;theuth\u0007\u001b[1;32mtheuth\u001b[0m 2.31.0\r\nopening store\u001b[K\r\n\r\n'
const view = (over: Partial<StartupInput>) => startupView({ ...BASE, ...over })
type Row = [id: string, status: string, detail: string | undefined]
const rows = (over: Partial<StartupInput>): Row[] => view(over).steps.map((s) => [s.id, s.status, s.detail])

test.each<[string, Partial<StartupInput>, Row[], string, boolean]>([
  ['not started: the engine step, no log yet', {}, [['engine', 'active', undefined], ['connecting', 'pending', undefined], ['loading', 'pending', undefined]], 'Starting theuth', false],
  ['engine starting: its detail is the last log line', { log: LOG }, [['engine', 'active', 'opening store'], ['connecting', 'pending', undefined], ['loading', 'pending', undefined]], 'Starting theuth', false],
  ['the shell has the connection, owned not read yet', { started: true, log: LOG }, [['engine', 'done', undefined], ['connecting', 'active', undefined], ['loading', 'pending', undefined]], 'Starting theuth', false],
  ['reused engine', { started: true, owned: false }, [['engine', 'done', 'Using the engine that is already running'], ['connecting', 'active', undefined], ['loading', 'pending', undefined]], 'Starting theuth', false],
  ['socket connecting, own engine', { started: true, owned: true, log: LOG }, [['engine', 'done', undefined], ['connecting', 'active', undefined], ['loading', 'pending', undefined]], 'Starting theuth', false],
  ['info pending', { started: true, owned: true, state: 'open' }, [['engine', 'done', undefined], ['connecting', 'done', undefined], ['loading', 'active', undefined]], 'Starting theuth', false],
  ['reconnecting after the engine was up', { started: true, owned: true, state: 'reconnecting', info: true }, [['engine', 'done', undefined], ['connecting', 'active', undefined], ['loading', 'pending', undefined]], 'Reconnecting to theuth', true],
  ['a first socket retrying is still the start', { started: true, owned: true, state: 'reconnecting' }, [['engine', 'done', undefined], ['connecting', 'active', undefined], ['loading', 'pending', undefined]], 'Starting theuth', true],
])('%s', (_, over, steps, title, relaunch) => {
  const v = view(over)
  expect(rows(over)).toEqual(steps)
  expect([v.title, v.failed, v.relaunch, v.logOpen]).toEqual([title, false, relaunch, false])
})

test.each<[string, Partial<StartupInput>, string]>([
  ['the engine did not start', { log: LOG }, 'engine'],
  ['the socket failed', { started: true, owned: true }, 'connecting'],
  ['info failed (older engine)', { started: true, owned: true, state: 'open' }, 'loading'],
])('error: %s — the active step fails with the error, the log opens, Relaunch shows', (_, over, failedId) => {
  const v = view({ ...over, error: 'boom' })
  expect(v.title).toBe('theuth could not start')
  expect([v.failed, v.relaunch, v.logOpen]).toEqual([true, true, true])
  const failed = v.steps.filter((s) => s.status === 'failed')
  expect(failed).toMatchObject([{ id: failedId, detail: 'boom', clip: false }])
  // Steps before it stay done, after it pending.
  const at = v.steps.findIndex((s) => s.id === failedId)
  expect(v.steps.slice(0, at).every((s) => s.status === 'done')).toBe(true)
  expect(v.steps.slice(at + 1).every((s) => s.status === 'pending')).toBe(true)
})

test('the launch log opens itself once the engine step has been active for 8 s, and stays open after', () => {
  expect(view({ log: LOG, waited: STUCK_MS - 1 }).logOpen).toBe(false)
  expect(view({ log: LOG, waited: STUCK_MS }).logOpen).toBe(true)
  expect(view({ log: LOG, waited: STUCK_MS, started: true, owned: true }).logOpen).toBe(true)
})

test('the last log line: escapes and controls stripped, a \\r redraw keeps what was drawn last, capped', () => {
  expect(lastLogLine('')).toBeUndefined()
  expect(lastLogLine('\r\n  \r\n\u001b[0m\r\n')).toBeUndefined()
  expect(lastLogLine(LOG)).toBe('opening store')
  expect(lastLogLine('a\nfetching 10%\rfetching 55%\r')).toBe('fetching 55%')
  expect(lastLogLine('\u001b[1;34m[oh-my-zsh]\u001b[0m Would you like to update? [Y/n] ')).toBe('[oh-my-zsh] Would you like to update? [Y/n]')
  expect(lastLogLine('x'.repeat(500))).toHaveLength(200)
})

test('the log pane text: \\r\\n and lone \\r handled, CSI/OSC/two-byte escapes and controls dropped, \\t and \\n kept', () => {
  expect(cleanLog('Last login: Tue Nov 17 11:02:41 on ttys004\r\n\u001b[1;34m[oh-my-zsh]\u001b[0m Would you like to update? [Y/n] ')).toBe(
    'Last login: Tue Nov 17 11:02:41 on ttys004\n[oh-my-zsh] Would you like to update? [Y/n] ',
  )
  expect(cleanLog('\u001b]0;theuth\u0007a\tb\u001b7\u001b8\r\nc\rd\u0000\u007f\u0008\n')).toBe('a\tb\ncd\n')
  // An unterminated OSC stops at the line end instead of eating the rest of the log.
  expect(cleanLog('\u001b]0;title\nnext line')).toBe('\nnext line')
})
