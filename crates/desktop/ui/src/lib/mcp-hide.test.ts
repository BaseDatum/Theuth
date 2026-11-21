import { expect, test } from 'vitest'
import type { McpServer, McpToolInfo } from '@/rpc/bindings/method.mcp_2dserver.output'
import { serverOps, serverState, toolOps, withDraftHides } from './mcp-hide'

const tool = (name: string, hidden = false, registered: string | null = name): McpToolInfo => ({ name, description: '', gate: null, risk: 'read_only', action: 'allow', registered, hidden })
const fs = (tools: McpToolInfo[]) => ({ name: 'fs', connected: true, tools }) as unknown as McpServer

test('a server’s checkbox: all shown, all hidden, or mixed; tools that are not registered do not count', () => {
  expect(serverState(fs([tool('a'), tool('b')]))).toBe('on')
  expect(serverState(fs([tool('a', true), tool('b', true), tool('off', false, null)]))).toBe('off')
  expect(serverState(fs([tool('a', true), tool('b')]))).toBe('mixed')
})

test('off hides the whole server by its `mcp:` entry; on clears the entry and every per-tool hide (registered names)', () => {
  const s = fs([tool('read', true, 'fs__read'), tool('write')])
  expect(serverOps(s, true)).toEqual([{ names: ['mcp:fs'], hidden: true }])
  expect(serverOps(s, false)).toEqual([{ names: ['mcp:fs', 'fs__read', 'write'], hidden: false }])
})

test('one tool back on inside a hidden server trades the server entry for hides of the others', () => {
  const off = fs([tool('a', true), tool('b', true), tool('c', true)])
  expect(toolOps(off, 'a', false)).toEqual([
    { names: ['mcp:fs', 'a'], hidden: false },
    { names: ['b', 'c'], hidden: true },
  ])
  const mixed = fs([tool('a', true), tool('b')])
  expect(toolOps(mixed, 'a', false)).toEqual([{ names: ['a'], hidden: false }])
  expect(toolOps(mixed, 'b', true)).toEqual([{ names: ['b'], hidden: true }])
})

test('a draft lays its pending hides over the engine’s list: the server entry hides every tool', () => {
  const s = fs([tool('a'), tool('b')])
  expect(serverState(withDraftHides(s, { 'mcp:fs': true }))).toBe('off')
  expect(serverState(withDraftHides(s, { a: true }))).toBe('mixed')
  expect(serverState(withDraftHides(s, { 'mcp:fs': false, a: false, b: true }))).toBe('mixed')
  expect(withDraftHides(s, undefined)).toBe(s)
})
