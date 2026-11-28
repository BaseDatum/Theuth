import { describe, expect, it, vi } from 'vitest'
import type { SessionSummary } from '@/rpc/bindings/method.agent.output'
import type { Task } from '@/rpc/bindings/method.roster.output'
import {
  buildTaskGraphContext,
  MAX_GRAPH_CONTEXT_TASKS,
  partitionGraphSessions,
  resolveTargetSession,
  sendTaskGraphMessage,
  taskGraphPlaceholder,
} from './composer-model'

const task = (id: string, over: Partial<Task> = {}): Task =>
  ({
    id,
    title: `Task ${id}`,
    state: 'todo',
    parent: null,
    blockers: [],
    blocks: [],
    project: 'alpha',
    brief: '',
    done_when: null,
    owner: { type: 'user' },
    priority: 'medium',
    attachments: [],
    accent: null,
    created_ms: 0,
    created_by: 'user',
    where: null,
    checkout: null,
    schedule: null,
    no_progress_count: 0,
    ...over,
  }) as unknown as Task

const sess = (id: string, over: Partial<SessionSummary> = {}): SessionSummary =>
  ({
    id,
    name: `name-${id}`,
    title: `Session ${id}`,
    project: null,
    archived: false,
    parent: null,
    kind: 'chat',
    updated_at: '2026-11-28T00:00:00Z',
    ...over,
  }) as SessionSummary

describe('TaskGraphComposer helpers', () => {
  it('builds compact TaskGraphSendContext capped at 80 tasks', () => {
    const many = Array.from({ length: 95 }, (_, i) =>
      task(`t-${i}`, {
        title: `Title ${i}`,
        state: i % 2 === 0 ? 'todo' : 'doing',
        parent: i === 0 ? null : 't-0',
        blockers: i > 1 ? [`t-${i - 1}`] : [],
      }),
    )
    const ctx = buildTaskGraphContext({ project: 'alpha', selected: 't-3', tasks: many })
    expect(ctx.project).toBe('alpha')
    expect(ctx.selected).toBe('t-3')
    expect(ctx.tasks).toHaveLength(MAX_GRAPH_CONTEXT_TASKS)
    expect(ctx.tasks?.[2]).toEqual({
      id: 't-2',
      title: 'Title 2',
      state: 'todo',
      parent: 't-0',
      blockers: ['t-1'],
    })
  })

  it('partitions sessions into project-bound and recent others', () => {
    const sessions = [
      sess('p-old', { project: 'alpha', updated_at: '2026-11-28T01:00:00Z' }),
      sess('p-new', { project: 'alpha', updated_at: '2026-11-28T05:00:00Z' }),
      sess('other-1', { project: 'beta', updated_at: '2026-11-28T04:00:00Z' }),
      sess('other-2', { project: null, updated_at: '2026-11-28T06:00:00Z' }),
      sess('archived', { project: 'alpha', archived: true, updated_at: '2026-11-28T09:00:00Z' }),
      sess('sub', { project: 'alpha', parent: 'p-new', updated_at: '2026-11-28T09:00:00Z' }),
      sess('obs', { project: 'alpha', kind: 'observed', updated_at: '2026-11-28T09:00:00Z' }),
    ]
    const forAlpha = partitionGraphSessions(sessions, 'alpha')
    expect(forAlpha.bound.map((s) => s.session)).toEqual(['p-new', 'p-old'])
    expect(forAlpha.recent.map((s) => s.session)).toEqual(['other-2', 'other-1'])

    const forAll = partitionGraphSessions(sessions, null, 2)
    expect(forAll.bound).toEqual([])
    expect(forAll.recent.map((s) => s.session)).toEqual(['other-2', 'p-new'])
  })

  it('resolves target session per project and respects explicit New session choice', () => {
    const bound = [{ session: 'p-1', title: 'Latest' }]
    expect(resolveTargetSession({}, 'alpha', bound)).toBe('p-1')
    expect(resolveTargetSession({}, 'alpha', [])).toBeNull()
    expect(resolveTargetSession({ alpha: null }, 'alpha', bound)).toBeNull()
    expect(resolveTargetSession({ alpha: 'custom' }, 'alpha', bound)).toBe('custom')
    expect(resolveTargetSession({ __all__: 's-all' }, null, [])).toBe('s-all')
  })

  it('formats project-aware placeholder text', () => {
    expect(taskGraphPlaceholder('alpha')).toBe('Ask an agent to add or organize tasks in alpha…')
    expect(taskGraphPlaceholder(null)).toBe('Ask an agent to add, link, or organize tasks…')
  })
})

describe('sendTaskGraphMessage', () => {
  it('creates a project-scoped session when session is null, sends context, then sends input', async () => {
    const order: string[] = []
    const createSession = vi.fn(async () => {
      order.push('create')
      return 'new-sess'
    })
    const sendGraphContext = vi.fn(async () => {
      order.push('context')
    })
    const sendInput = vi.fn(async () => {
      order.push('send_input')
    })
    const steer = vi.fn(async () => {
      order.push('steer')
    })

    const out = await sendTaskGraphMessage(
      { createSession, sendGraphContext, sendInput, steer },
      {
        session: null,
        running: false,
        project: 'alpha',
        selected: 't-1',
        tasks: [task('t-1')],
        text: 'Add a follow-up task',
        files: ['f1'],
        clientId: 'cid-1',
        pick: { model: 'm/fast', effort: 'high' },
      },
    )

    expect(out).toBe('new-sess')
    expect(order).toEqual(['create', 'context', 'send_input'])
    expect(createSession).toHaveBeenCalledWith(expect.objectContaining({ project: 'alpha', model: 'm/fast', effort: 'high' }))
    expect(sendGraphContext).toHaveBeenCalledWith('new-sess', {
      project: 'alpha',
      selected: 't-1',
      tasks: [{ id: 't-1', title: 'Task t-1', state: 'todo', parent: null, blockers: [] }],
    })
    expect(sendInput).toHaveBeenCalledWith('new-sess', 'Add a follow-up task', ['f1'], 'cid-1')
    expect(steer).not.toHaveBeenCalled()
  })

  it('steers a running session and tags an untagged session with the active project (best-effort)', async () => {
    const order: string[] = []
    const setSessionProject = vi.fn(async () => {
      order.push('set_project')
    })
    const sendGraphContext = vi.fn(async () => {
      order.push('context')
    })
    const steer = vi.fn(async () => {
      order.push('steer')
    })
    const sendInput = vi.fn()

    const out = await sendTaskGraphMessage(
      {
        createSession: vi.fn(),
        setSessionProject,
        sendGraphContext,
        sendInput,
        steer,
      },
      {
        session: 's-1',
        existingProject: null,
        running: true,
        project: 'alpha',
        selected: null,
        tasks: [],
        text: 'Also link them',
        files: [],
        clientId: 'cid-2',
      },
    )

    expect(out).toBe('s-1')
    expect(order).toEqual(['set_project', 'context', 'steer'])
    expect(setSessionProject).toHaveBeenCalledWith('s-1', 'alpha')
    expect(steer).toHaveBeenCalledWith('s-1', 'Also link them', 'cid-2', [])
    expect(sendInput).not.toHaveBeenCalled()
  })

  it('does not overwrite project when existing session already has one', async () => {
    const setSessionProject = vi.fn(async () => {})
    const sendInput = vi.fn(async () => {})
    await sendTaskGraphMessage(
      {
        createSession: vi.fn(),
        setSessionProject,
        sendInput,
        steer: vi.fn(),
      },
      {
        session: 's-1',
        existingProject: 'beta',
        running: false,
        project: 'alpha',
        selected: null,
        tasks: [],
        text: 'Hello',
        files: [],
        clientId: 'cid-4',
      },
    )
    expect(setSessionProject).not.toHaveBeenCalled()
    expect(sendInput).toHaveBeenCalledOnce()
  })

  it('degrades gracefully when setSessionProject or task_graph_send_context fails or is unsupported', async () => {
    const sendInput = vi.fn(async () => {})
    const out = await sendTaskGraphMessage(
      {
        createSession: vi.fn(),
        setSessionProject: vi.fn(async () => {
          throw new Error('unsupported')
        }),
        sendGraphContext: vi.fn(async () => {
          throw new Error('method not found')
        }),
        sendInput,
        steer: vi.fn(),
      },
      {
        session: 's-2',
        existingProject: null,
        running: false,
        project: 'alpha',
        selected: null,
        tasks: [],
        text: 'Create tasks',
        files: [],
        clientId: 'cid-3',
      },
    )
    expect(out).toBe('s-2')
    expect(sendInput).toHaveBeenCalledWith('s-2', 'Create tasks', [], 'cid-3')
  })
})
