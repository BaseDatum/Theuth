// The task card: its body draws the task's live record from the task store (and "task deleted" once the store has no
// such task); on the canvas a click on its body (no drag) opens the task in a pane through the shared view-tile opener.
import '../settings/test-dom'
import { act, createElement as h } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import * as Y from 'yjs'
import type { Task } from '@/rpc/bindings/method.roster.output'

const tasks = vi.hoisted(() => ({ list: { data: undefined as unknown, isError: false, error: null as unknown } }))
vi.mock('@/components/tasks/queries', () => ({
  EMPTY_FILTER: { owner: null, state: null, engine: null, project: null },
  useTasks: () => tasks.list,
  useLiveTasks: () => {},
  useMoveTask: () => ({ mutate: vi.fn(), isPending: false }),
  useCloseDone: () => ({ mutate: vi.fn(), isPending: false }),
  doneBecause: () => ({ type: 'intent_met' }),
}))
vi.mock('@/components/tiles/open', () => ({ openViewFromCard: vi.fn(() => 'tile:view') }))

import { openViewFromCard } from '@/components/tiles/open'
import type { CanvasHost } from './canvas/api'
import Canvas from './canvas/Canvas'
import TaskCardBody from './canvas/cards/TaskCard'
import { addCard } from './canvas/ops'
import { createWhiteboardStore } from './canvas/store'

const task = (over: Partial<Task> = {}): Task =>
  ({
    id: 'T1', title: 'Fix login', state: 'todo', owner: { type: 'agent', name: 'ops' }, priority: 'high', blockers: ['T0', 'T9'],
    resolved_blockers: ['T9'], done_when: 'Login works on iOS\nand Android', reviewer: null, checkout: null, ...over,
  }) as Task

let root: Root
let el: HTMLDivElement
beforeEach(() => {
  el = document.createElement('div')
  document.body.append(el)
  root = createRoot(el)
  tasks.list = { data: [task()], isError: false, error: null }
  vi.mocked(openViewFromCard).mockClear()
})
afterEach(() => {
  act(() => root.unmount())
  el.remove()
  document.body.innerHTML = ''
})

const q = (sel: string) => el.querySelector<HTMLElement>(sel)

describe('task card body', () => {
  it('renders the live fields and follows the task as it changes', async () => {
    await act(async () => root.render(h(TaskCardBody, { task: 'T1' })))
    expect(q('[data-task-title]')!.textContent).toBe('Fix login')
    expect(q('[data-task-badge]')!.textContent).toBe("queued for ops's desk")
    expect(q('[data-task-owner]')!.textContent).toBe('ops')
    expect(q('[data-task-priority]')!.textContent).toBe('high')
    expect(q('[data-task-blockers]')!.textContent).toBe('1 blocker')
    expect(q('[data-task-done-when]')!.textContent).toBe('Login works on iOS')

    tasks.list = { data: [task({ title: 'Fix login (iOS)', state: 'review', blockers: [], done_when: null })], isError: false, error: null }
    await act(async () => root.render(h(TaskCardBody, { task: 'T1' })))
    expect(q('[data-task-title]')!.textContent).toBe('Fix login (iOS)')
    expect(q('[data-task-badge]')!.textContent).toBe('review')
    expect(q('[data-task-blockers]')).toBeNull()
    expect(q('[data-task-done-when]')).toBeNull()
  })

  it('shows a deleted task dimmed', async () => {
    tasks.list = { data: [task({ id: 'other' })], isError: false, error: null }
    await act(async () => root.render(h(TaskCardBody, { task: 'T1' })))
    const card = q('[data-task-card="T1"]')!
    expect(card.dataset.taskStatus).toBe('deleted')
    expect(card.textContent).toContain('task deleted')
    expect(card.className).toContain('opacity-50')
  })

  it('waits for the list rather than calling a task deleted', async () => {
    tasks.list = { data: undefined, isError: false, error: null }
    await act(async () => root.render(h(TaskCardBody, { task: 'T1' })))
    expect(q('[data-task-card="T1"]')!.dataset.taskStatus).toBe('loading')
  })
})

describe('task card on the canvas', () => {
  const HOST: CanvasHost = { doc: 'd', blobUrl: async () => null, blobPut: async () => {}, capture: async () => null, describePoint: async () => null, liveBudget: 8 }
  function pointer(target: Element, type: string, x: number, y: number) {
    const e = new MouseEvent(type, { bubbles: true, cancelable: true, button: 0, buttons: type === 'pointerup' ? 0 : 1, clientX: x, clientY: y })
    Object.defineProperties(e, { pointerType: { value: 'mouse' }, pointerId: { value: 1 } })
    target.dispatchEvent(e)
  }

  it('a click on the body opens the task in a pane; a drag (a board pan) does not', async () => {
    Element.prototype.setPointerCapture ??= () => {}
    const doc = new Y.Doc()
    const id = addCard(doc, { kind: 'task', title: 'Fix login', x: 0, y: 0, w: 280, h: 168, source: { t: 'task', task: 'T1' } })
    await act(async () => root.render(h(Canvas, { provider: { doc } as never, store: createWhiteboardStore(), host: HOST })))
    const body = document.querySelector(`[data-card="${id}"] [data-body]`)!
    expect(body).not.toBeNull()

    await act(async () => {
      pointer(body, 'pointerdown', 40, 40)
      pointer(body, 'pointermove', 140, 90)
      pointer(body, 'pointerup', 140, 90)
    })
    expect(openViewFromCard).not.toHaveBeenCalled()

    await act(async () => {
      pointer(body, 'pointerdown', 40, 40)
      pointer(body, 'pointerup', 42, 41)
    })
    expect(openViewFromCard).toHaveBeenCalledTimes(1)
    expect(vi.mocked(openViewFromCard).mock.calls[0]![0]).toEqual({ path: '/tasks/T1', title: 'Fix login' })
  })
})
