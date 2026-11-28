import { describe, expect, test, vi } from 'vitest'
import type { Task } from '@/rpc/bindings/method.roster.output'
import {
  HARNESSES,
  NEW_TASK_EVENT,
  NewTaskHost,
  type NewTaskModalDetail,
  ProjectCombobox,
  TaskModal,
  blockersOf,
  dependentsOf,
  inferProjectFromLocation,
  openNewTaskModal,
} from './TaskModal'

function stubTask(id: string, blockers: string[] = []): Pick<Task, 'id' | 'blockers'> {
  return { id, blockers }
}

describe('TaskModal public contract', () => {
  test('exports the exact shared public API', () => {
    expect(typeof openNewTaskModal).toBe('function')
    expect(typeof TaskModal).toBe('function')
    expect(typeof NewTaskHost).toBe('function')
    expect(typeof ProjectCombobox).toBe('function')
    expect(HARNESSES).toEqual(['native', 'omp', 'opencode', 'claude'])
  })

  test('openNewTaskModal dispatches the global event with its detail payload', () => {
    const hadWindow = 'window' in globalThis
    const prevWindow = (globalThis as { window?: unknown }).window
    const target = new EventTarget()
    if (!hadWindow) (globalThis as { window: unknown }).window = target
    const win = (globalThis as { window: EventTarget }).window
    const received: Array<NewTaskModalDetail | undefined> = []
    const onEvent = (ev: Event) => {
      received.push((ev as CustomEvent<NewTaskModalDetail | undefined>).detail)
    }
    win.addEventListener(NEW_TASK_EVENT, onEvent)
    try {
      const onCreated = vi.fn()
      openNewTaskModal({ project: 'theuth', parent: 't-root', state: 'backlog', onCreated })
      openNewTaskModal()
      expect(received).toHaveLength(2)
      expect(received[0]).toEqual({ project: 'theuth', parent: 't-root', state: 'backlog', onCreated })
      expect(received[1]).toBeNull()
    } finally {
      win.removeEventListener(NEW_TASK_EVENT, onEvent)
      if (!hadWindow) delete (globalThis as { window?: unknown }).window
      else (globalThis as { window?: unknown }).window = prevWindow
    }
  })
})

describe('inferProjectFromLocation', () => {
  test('extracts project from /projects/$name routes', () => {
    expect(inferProjectFromLocation('/projects/theuth')).toBe('theuth')
    expect(inferProjectFromLocation('/projects/my%20project/tasks')).toBe('my project')
    expect(inferProjectFromLocation('/projects/')).toBeNull()
  })

  test('extracts project from /tasks and /tasks/graph search params', () => {
    expect(inferProjectFromLocation('/tasks', { project: 'alpha' })).toBe('alpha')
    expect(inferProjectFromLocation('/tasks/', { project: 'alpha' })).toBe('alpha')
    expect(inferProjectFromLocation('/tasks/graph', { project: '2026' })).toBe('2026')
    expect(inferProjectFromLocation('/tasks/graph/', { project: 2026 })).toBe('2026')
    expect(inferProjectFromLocation('/tasks', { project: '' })).toBeNull()
    expect(inferProjectFromLocation('/tasks', { project: '   ' })).toBeNull()
    expect(inferProjectFromLocation('/tasks', {})).toBeNull()
  })

  test('returns null on unrelated routes', () => {
    expect(inferProjectFromLocation('/c/123', { project: 'ignored' })).toBeNull()
    expect(inferProjectFromLocation('/attention')).toBeNull()
  })
})

describe('cycle-safe dependency traversal', () => {
  test('dependentsOf finds direct and transitive dependents without hanging on cycles', () => {
    const tasks = [
      stubTask('a', []),
      stubTask('b', ['a']),
      stubTask('c', ['b']),
      stubTask('d', ['c', 'person:sky']),
      stubTask('x', ['y']),
      stubTask('y', ['x']),
    ]
    expect([...dependentsOf(tasks, 'a')].sort()).toEqual(['b', 'c', 'd'])
    expect([...dependentsOf(tasks, 'c')]).toEqual(['d'])
    expect([...dependentsOf(tasks, 'd')]).toEqual([])
    expect([...dependentsOf(tasks, 'x')]).toEqual(['y'])
  })

  test('blockersOf finds direct and transitive blockers without hanging on cycles', () => {
    const tasks = [
      stubTask('a', []),
      stubTask('b', ['a']),
      stubTask('c', ['b']),
      stubTask('d', ['c', 'decision:launch']),
      stubTask('x', ['y']),
      stubTask('y', ['x']),
    ]
    expect([...blockersOf(tasks, 'd')].sort()).toEqual(['a', 'b', 'c'])
    expect([...blockersOf(tasks, 'b')]).toEqual(['a'])
    expect([...blockersOf(tasks, 'a')]).toEqual([])
    expect([...blockersOf(tasks, 'x')]).toEqual(['y'])
  })
})
