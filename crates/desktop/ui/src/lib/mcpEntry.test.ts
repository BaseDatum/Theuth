import { expect, test } from 'vitest'
import { gitUrl, mcpEntry } from './mcpEntry'

test('named object, {name: entry} and mcpServers wrapper all yield the same server', () => {
  const want = { name: 'fs', entry: { command: 'npx', args: ['-y', 'fs'] } }
  expect(mcpEntry('{"name":"fs","command":"npx","args":["-y","fs"]}')).toEqual(want)
  expect(mcpEntry('{"fs":{"command":"npx","args":["-y","fs"]}}')).toEqual(want)
  expect(mcpEntry('{"mcpServers":{"fs":{"command":"npx","args":["-y","fs"]}}}')).toEqual(want)
})

test('rejects non-objects, nameless server objects and multi-server maps', () => {
  expect(() => mcpEntry('[]')).toThrow('expected a JSON object')
  expect(() => mcpEntry('{"command":"x"}')).toThrow('needs a "name" field')
  expect(() => mcpEntry('{"a":{"url":"u"},"b":{"url":"v"}}')).toThrow('exactly one server')
  expect(() => mcpEntry('{"a":1}')).toThrow('exactly one server')
})

test('git urls: .git suffix or a known host', () => {
  expect(gitUrl('https://github.com/x/y')).toBe(true)
  expect(gitUrl('git@example.org:x/y.git')).toBe(true)
  expect(gitUrl('{"fs":{}}')).toBe(false)
})
