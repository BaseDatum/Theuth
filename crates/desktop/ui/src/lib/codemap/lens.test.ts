import { expect, test } from 'vitest'
import { follow, initialLens, lensFor, onCodeToolRan, pin, replay, type Lens } from './lens'
import type { CodeTool, CodeToolRan, CodeToolSummary } from './types'

const ok: CodeToolSummary = { rows: 1, dropped: 0, is_error: false, stale: false, pending: 0, truncated_walk: false, suspect_root: false, hint: null }
const def = { moniker: 'rust:a/b.rs#f(function)', path: 'a/b.rs', line_start: 1, line_end: 9, name: 'f', kind: 'function' }
const ev = (tool: CodeTool, args: Record<string, unknown>, summary: Partial<CodeToolSummary> = {}, call_id = 'c1'): CodeToolRan => ({
  type: 'CodeToolRan', session: 's', root: '/r', call_id, tool, args, summary: { ...ok, ...summary }, seq: 1,
})

test('lensFor: every §6.1 transition', () => {
  const rows: [CodeTool, Record<string, unknown>, Partial<CodeToolSummary>, Lens, unknown, string | null][] = [
    ['code_map', { focus_files: ['crates/tools'], focus_idents: ['run'] }, {}, 'here', { here: { focus: ['crates/tools', 'run'], edited: null } }, null],
    ['code_map', { focus: ['x'] }, {}, 'here', { here: { focus: ['x'], edited: null } }, null],
    ['code_find', { name: 'start' }, {}, 'here', { here: { focus: ['start'], edited: null } }, null],
    ['code_find', { query: 'q' }, {}, 'here', { here: { focus: ['q'], edited: null } }, null],
    ['code_outline', { path: 'a.rs' }, { path: 'b/a.rs' }, 'here', { here: { focus: ['b/a.rs'], edited: null } }, null],
    ['code_outline', { path: 'a.rs' }, {}, 'here', { here: { focus: ['a.rs'], edited: null } }, null],
    ['code_refs', { symbol: 'f' }, { definition: def }, 'fit', { fit: { symbol: def.moniker } }, def.moniker],
    ['code_symbol', { name: 'f' }, { definition: def }, 'fit', { fit: { symbol: def.moniker } }, def.moniker],
    // candidates / definition null → fit with the asked symbol, no highlight
    ['code_refs', { symbol: 'f' }, { definition: null }, 'fit', { fit: { symbol: 'f' } }, null],
    ['code_symbol', { name: 'g' }, {}, 'fit', { fit: { symbol: 'g' } }, null],
    ['code_impact', { git_range: 'a..b', files: ['x.rs'], include_tests: false }, {}, 'change', { change: { git_range: 'a..b', files: ['x.rs'], include_tests: false } }, null],
    ['code_impact', {}, {}, 'change', { change: { git_range: null, files: null, include_tests: true } }, null],
  ]
  for (const [tool, args, summary, lens, params, focus] of rows) {
    expect(lensFor(tool, args, { ...ok, ...summary }), tool).toEqual({ lens, params, focus })
  }
  for (const tool of ['code_map', 'code_find', 'code_outline', 'code_refs', 'code_symbol', 'code_impact'] as CodeTool[]) {
    expect(lensFor(tool, { symbol: 'x' }, { ...ok, is_error: true }), tool).toBeNull()
  }
})

test('auto follows each tool; is_error never switches', () => {
  let s = initialLens()
  s = onCodeToolRan(s, ev('code_refs', { symbol: 'f' }, { definition: def }, 'c1'))
  expect(s).toMatchObject({ mode: 'auto', lens: 'fit', auto: 'fit', focus: def.moniker, callId: 'c1', params: { fit: { symbol: def.moniker } } })
  s = onCodeToolRan(s, ev('code_impact', {}, {}, 'c2'))
  expect(s).toMatchObject({ lens: 'change', auto: 'change', focus: null, callId: 'c2' })
  expect(s.params.fit).toEqual({ symbol: def.moniker })
  const before = s
  s = onCodeToolRan(s, ev('code_map', { focus_files: ['x'] }, { is_error: true }, 'c3'))
  expect(s).toBe(before)
  s = onCodeToolRan(s, ev('code_map', { focus_files: ['x'] }, {}, 'c4'))
  expect(s).toMatchObject({ lens: 'here', auto: 'here', callId: 'c4', params: { here: { focus: ['x'], edited: null } } })
})

test('pinned keeps the lens and updates auto/params; follow returns to auto', () => {
  let s = pin(initialLens(), 'change', { change: { git_range: null, files: ['a.rs'], include_tests: true } })
  expect(s).toMatchObject({ mode: 'pinned', lens: 'change', auto: 'here', callId: null, params: { change: { files: ['a.rs'] } } })
  s = onCodeToolRan(s, ev('code_refs', { symbol: 'f' }, { definition: def }, 'c9'))
  expect(s).toMatchObject({ mode: 'pinned', lens: 'change', auto: 'fit', focus: null, callId: null, params: { fit: { symbol: def.moniker } } })
  expect(pin(s, 'fit', undefined, 'm').focus).toBe('m')
  expect(pin(s, 'fit').focus).toBe(s.focus)
  s = follow(s)
  expect(s).toMatchObject({ mode: 'auto', lens: 'fit' })
})

test('replay pins the row lens with its callId; error rows replay nothing', () => {
  const s0 = initialLens()
  const s = replay(s0, { tool: 'code_refs', args: { symbol: 'f' }, summary: { ...ok, definition: def }, callId: 'c7' })
  expect(s).toMatchObject({ mode: 'pinned', lens: 'fit', auto: 'here', focus: def.moniker, callId: 'c7', params: { fit: { symbol: def.moniker } } })
  expect(replay(s0, { tool: 'code_refs', args: { symbol: 'f' }, summary: { ...ok, is_error: true }, callId: 'c8' })).toBe(s0)
})
