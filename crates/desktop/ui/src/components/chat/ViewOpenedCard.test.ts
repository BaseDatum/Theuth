import { expect, test } from 'vitest'
import type { Entry } from '@/store/sessions'
import { openLabel, viewCall } from './ViewOpenedCard'

type Tool = Extract<Entry, { kind: 'tool' }>
const LEAF = 'tile:view::%2Ftasks%2Fgraph'
const tool = (over: Partial<Tool>): Tool => ({ id: 'e1', kind: 'tool', callId: 'c1', name: 'show_view', args: {}, pending: false, child: false, ...over })
const result = (output: string, isError = false) => ({ output, isError, files: [] })

test('a show_view receipt names the title, the route and the pane; reused says so', () => {
  const out = `view opened · Project · graph · /tasks/graph · tile ${LEAF}\nOpened Project · graph (/tasks/graph) in pane ${LEAF}`
  expect(viewCall(tool({ result: result(out) }))).toEqual({ title: 'Project · graph', path: '/tasks/graph', leaf: LEAF, reused: false })
  const reused = `view reused · Tasks · /tasks?tab=open · tile tile:view::%2Ftasks\nReused Tasks (/tasks?tab=open) in pane tile:view::%2Ftasks`
  expect(viewCall(tool({ result: result(reused) }))).toEqual({ title: 'Tasks', path: '/tasks?tab=open', leaf: 'tile:view::%2Ftasks', reused: true })
})

test('a receipt written before the pinned line is read from its human line', () => {
  expect(viewCall(tool({ result: result(`Opened Costs (/costs) in pane tile:view::%2Fcosts`) }))).toEqual({ title: 'Costs', path: '/costs', leaf: 'tile:view::%2Fcosts', reused: false })
  expect(viewCall(tool({ result: result(`Reused My (odd) view (/notes/x) in pane tile:view::%2Fnotes%2Fx`) }))).toMatchObject({ title: 'My (odd) view', path: '/notes/x', reused: true })
})

test('a pending, failed or unparsable call has no pane; its arguments name it', () => {
  expect(viewCall(tool({ pending: true, args: { path: '/tasks', placement: 'tab' } }))).toEqual({ title: '/tasks', path: '/tasks', leaf: null, reused: false })
  expect(viewCall(tool({ args: { path: '/x', title: 'X' }, result: result('show_view: no desktop app connected', true) }))).toEqual({ title: 'X', path: '/x', leaf: null, reused: false })
  expect(viewCall(tool({ args: { path: '/x' }, result: result('something else') })).leaf).toBeNull()
})

test('the open button reads Focus while the pane is open, Reopen once closed, Open where nothing tiles', () => {
  expect(openLabel(true, true)).toBe('Focus')
  expect(openLabel(true, false)).toBe('Reopen')
  expect(openLabel(false, false)).toBe('Open')
  expect(openLabel(false, true)).toBe('Open')
})
