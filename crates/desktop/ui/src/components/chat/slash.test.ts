import { expect, test, vi } from 'vitest'
import type { Api } from '@/rpc/api'
import { type Ctx, isCommand, run } from './slash'

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

test('bare /skills and /mcp open the Library pages, carrying the chat they came from', async () => {
  const navigate = vi.fn().mockResolvedValue(undefined)
  const ctx = { api: {} as Api, session: 'abc', parent: null, navigate, reload: async () => {} } as unknown as Ctx
  await run('/skills', ctx)
  expect(navigate).toHaveBeenLastCalledWith({ to: '/skills', search: { session: 'abc' } })
  await run('/mcp', ctx)
  expect(navigate).toHaveBeenLastCalledWith({ to: '/tools', search: { session: 'abc' } })
  await run('/skills', { ...ctx, session: null })
  expect(navigate).toHaveBeenLastCalledWith({ to: '/skills', search: {} })
})
