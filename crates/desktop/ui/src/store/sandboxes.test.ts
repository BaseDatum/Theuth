import { QueryClient } from '@tanstack/react-query'
import { expect, test } from 'vitest'
import type { SandboxInfo } from '@/rpc/bindings/SandboxInfo'
import { LOG_CAP, type ProgressLine, SANDBOXES_KEY, appendLog, applySandboxEvent, guestPath, remove, sandboxLogKey, stateLabel, upsert } from './sandboxes'

const row = (name: string, state: SandboxInfo['state'] = { state: 'ready' }): SandboxInfo => ({ name, backend: 'vm', state, engine: null, vm: null, created_ms: 1, updated_ms: 1 })

test('Changed upserts by name (append when new, replace in place when known); Removed deletes the row and its log', () => {
  const qc = new QueryClient()
  applySandboxEvent(qc, { type: 'Changed', sandbox: row('a') })
  applySandboxEvent(qc, { type: 'Changed', sandbox: row('b', { state: 'creating', step: 'create' }) })
  applySandboxEvent(qc, { type: 'Changed', sandbox: row('a', { state: 'running' }) })
  expect(qc.getQueryData<SandboxInfo[]>(SANDBOXES_KEY)?.map((s) => [s.name, s.state.state])).toEqual([
    ['a', 'running'],
    ['b', 'creating'],
  ])
  applySandboxEvent(qc, { type: 'Progress', name: 'b', step: 'create', line: 'pulling', pct: 10, session: null, call_id: null })
  expect(qc.getQueryData<ProgressLine[]>(sandboxLogKey('b'))).toHaveLength(1)
  applySandboxEvent(qc, { type: 'Removed', name: 'b' })
  expect(qc.getQueryData<SandboxInfo[]>(SANDBOXES_KEY)?.map((s) => s.name)).toEqual(['a'])
  expect(qc.getQueryData(sandboxLogKey('b'))).toBeUndefined()
  // Pure forms behave the same on an empty cache.
  expect(upsert(undefined, row('z'))).toHaveLength(1)
  expect(remove(undefined, 'z')).toEqual([])
})

test('the per-name log keeps the newest LOG_CAP lines', () => {
  let log: ProgressLine[] | undefined
  for (let i = 0; i < LOG_CAP + 7; i++) log = appendLog(log, { step: 's', line: `l${i}`, pct: null })
  expect(log).toHaveLength(LOG_CAP)
  expect(log?.[0]?.line).toBe('l7')
  expect(log?.[LOG_CAP - 1]?.line).toBe(`l${LOG_CAP + 6}`)
})

test('labels: creating and failed carry their step; a share lands under /shared/<basename>', () => {
  expect(stateLabel({ state: 'creating', step: 'pair' })).toBe('creating · pair')
  expect(stateLabel({ state: 'failed', step: 'verify', error: 'x' })).toBe('failed at verify')
  expect(stateLabel({ state: 'paused' })).toBe('paused')
  expect(guestPath('/Users/sky/Develop/')).toBe('/shared/Develop')
  expect(guestPath('/')).toBe('/shared/root')
})
