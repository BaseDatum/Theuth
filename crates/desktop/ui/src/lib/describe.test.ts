import { expect, test } from 'vitest'
import { codeDetails, describe, resultLine } from './describe'

test('shell shows the command, continuation lines indented', () => {
  expect(describe('shell', { command: ' ls -la\nwc -l \n', timeout: 5 })).toEqual(['$ ls -la', '  wc -l'])
})

test('code details hide source and choose its syntax', () => {
  expect(codeDetails('python', { code: 'print("hi")' })).toEqual({ code: 'print("hi")', language: 'python', args: {} })
  expect(codeDetails('shell', { command: 'echo hi', timeout: 5 })).toEqual({ code: 'echo hi', language: 'bash', args: { timeout: 5 } })
  expect(codeDetails('start', { command: 'bun dev', name: 'web' })).toEqual({ code: 'bun dev', language: 'bash', args: { name: 'web' } })
  expect(codeDetails('write_file', { path: 'App.tsx', content: 'export default 1' })).toEqual({
    code: 'export default 1',
    language: 'tsx',
    args: { path: 'App.tsx' },
  })
  expect(codeDetails('read_file', { path: 'App.tsx' })).toBeNull()
})

test('key: value per argument, object order kept', () => {
  expect(describe('read_file', { path: 'src/a.rs', lines: [1, 20], all: false })).toEqual([
    'path: src/a.rs',
    'lines: [1,20]',
    'all: false',
  ])
})

test('long or multi-line strings collapse to their size; long json is cut', () => {
  expect(describe('write_file', { path: 'x', content: 'héllo\nworld\n' })).toEqual([
    'path: x',
    'content: (2 lines, 13 bytes)',
  ])
  expect(describe('t', { s: 'a'.repeat(121) })).toEqual(['s: (1 lines, 121 bytes)'])
  const [line] = describe('t', { v: Array.from({ length: 60 }, (_, i) => i) })
  expect(line).toHaveLength(`v: `.length + 120 + 1)
  expect(line?.endsWith('…')).toBe(true)
})

test('null is empty, a non-object is its json', () => {
  expect(describe('t', null)).toEqual([])
  expect(describe('t', [1, 2])).toEqual(['[1,2]'])
})

test('a whiteboard source is its kind and indented fields, not json', () => {
  expect(describe('wb_place', { kind: 'sticky', title: 'From the agent', source: { t: 'sticky', text: 'Placed by the agent\nline 2' } })).toEqual([
    'kind: sticky',
    'title: From the agent',
    'source: sticky',
    '  text: (2 lines, 26 bytes)',
  ])
  expect(describe('wb_update', { card: 'c3', source: { t: 'excerpt', path: 'a.ts', start: 1, end: 4 } })).toEqual(['card: c3', 'source: excerpt', '  path: a.ts', '  start: 1', '  end: 4'])
})

test('resultLine', () => {
  expect(resultLine('ok')).toBe('ok')
  expect(resultLine('a\nb\nc\n')).toBe('a  (+2 lines)')
})
