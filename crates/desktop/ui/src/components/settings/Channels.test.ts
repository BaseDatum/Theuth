import { expect, test } from 'vitest'
import type { TelegramEvent } from '@/rpc/bindings/TelegramEvent'
import type { TelegramStatus } from '@/rpc/bindings/TelegramStatus'
import { mmss, secondsLeft, telegramId, telegramState } from './Channels'

const base = (state: TelegramEvent | null, polling = true, bot = true): TelegramStatus => ({
  state,
  bot: bot ? { id: 7n, username: 'omp_bot', topics: true, users_may_create_topics: true } : null,
  polling,
  owner: null,
  allowed: [],
  groups: [],
  threads: [],
  open: 0,
  calls: {},
  dropped_updates: 0n,
})

test('telegramState: one phrase per variant (98 §10.5)', () => {
  const cases: [TelegramEvent, string][] = [
    [{ type: 'Unconfigured' }, 'no bot token'],
    [{ type: 'Identity', bot: 'omp_bot', topics: false }, 'connected as @omp_bot · topics off'],
    [{ type: 'Connected', bot: 'omp_bot' }, 'connected as @omp_bot · topics on'],
    [{ type: 'Disconnected', reason: 'timeout' }, 'disconnected: timeout'],
    [{ type: 'Conflict' }, 'another poller holds this token'],
    [{ type: 'RateLimited', method: 'sendMessage', retry_after_secs: 5n }, 'rate limited on sendMessage, retry in 5s'],
    [{ type: 'StreamFallback', session: 's1', reason: 'draft failed' }, 'connected'],
    [{ type: 'Unreachable', chat: -100n, reason: 'kicked' }, 'chat -100 unreachable: kicked'],
    [{ type: 'PairingLink', url: 'https://t.me/omp_bot?start=x', expires_at_ms: 0n }, 'waiting for the owner to tap Start'],
    [{ type: 'Paired', user: 42n, name: 'Sky' }, 'paired with Sky'],
    [{ type: 'DaemonOnly' }, 'daemon only'],
  ]
  for (const [state, phrase] of cases) expect(telegramState(base(state))).toBe(phrase)
  expect(telegramState(base(null))).toBe('unknown')
})

test('telegramState: idle suffix only for a configured bot that is not polling', () => {
  expect(telegramState(base({ type: 'Connected', bot: 'omp_bot' }, false))).toBe('connected as @omp_bot · topics on · idle (hub elsewhere)')
  expect(telegramState(base({ type: 'Connected', bot: 'omp_bot' }, false, false))).toBe('connected as @omp_bot')
  expect(telegramState(base({ type: 'Unconfigured' }, false, false))).toBe('no bot token')
})

test('telegramId: digits only; a leading minus only for chats', () => {
  expect(telegramId(' 123456789 ', 'user')).toBe(123456789)
  expect(telegramId('-1001234567890', 'chat')).toBe(-1001234567890)
  for (const bad of ['', '-1', '12a', '@sky', '+1 555', '1.5']) expect(telegramId(bad, 'user')).toBeNull()
  for (const bad of ['', '-', '--100', '1e3']) expect(telegramId(bad, 'chat')).toBeNull()
})

test('countdown: whole seconds, floored at 0, rendered m:ss', () => {
  expect(secondsLeft(10_000, 0)).toBe(10)
  expect(secondsLeft(10_001, 0)).toBe(11)
  expect(secondsLeft(5_000, 9_000)).toBe(0)
  expect(mmss(600)).toBe('10:00')
  expect(mmss(65)).toBe('1:05')
  expect(mmss(0)).toBe('0:00')
})
