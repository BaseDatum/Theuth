import { expect, test } from 'vitest'
import type { AgentSummary } from '@/rpc/bindings/AgentSummary'
import { hiring, managerNames, proposed } from './hiring'

const s = (name: string, extra: Partial<AgentSummary> = {}, file: Partial<AgentSummary['file']> = {}): AgentSummary => ({
  file: { name, title: name, reports_to: 'theuth', model: null, effort: null, tools: null, mcp: [], budget: { type: 'none' }, capabilities: null, paused: false, proposed_by: null, body: 'x', ...file },
  status: 'idle', desk: null, open_tasks: 0, spent_month: 0, ...extra,
})

test('the inbox lists proposed agents only (by status or proposed_by), sorted by name', () => {
  const list = [s('zed', { status: 'proposed' }), s('ops'), s('amy', {}, { proposed_by: 'agent:cto', paused: true }), s('theuth')]
  expect(proposed(list).map((a) => a.file.name)).toEqual(['amy', 'zed'])
  expect(proposed(undefined)).toEqual([])
})

test('approve calls agent_approve; decline calls agent_remove with the trimmed note or null', async () => {
  const calls: unknown[] = []
  const h = hiring({
    agent_approve: async (name) => {
      calls.push(['approve', name])
      return null
    },
    agent_remove: async (name, note) => {
      calls.push(['remove', name, note])
      return null
    },
  })
  await h.approve('ops')
  await h.reject('amy', '  we have ops covered ')
  await h.reject('zed', '   ')
  expect(calls).toEqual([
    ['approve', 'ops'],
    ['remove', 'amy', 'we have ops covered'],
    ['remove', 'zed', null],
  ])
})

test('managers are theuth plus every other roster name, never the agent itself', () => {
  expect(managerNames([s('theuth'), s('sre'), s('cto')], 'sre')).toEqual(['theuth', 'cto'])
  expect(managerNames(undefined, 'x')).toEqual(['theuth'])
})
