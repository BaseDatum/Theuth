import { describe, expect, it } from 'vitest'
import { leavesCard, taskGraphPath, viewCardPath, viewCardTitle } from './view-card'

describe('view card paths', () => {
  it('takes the app view routes, query kept', () => {
    expect(viewCardPath(taskGraphPath('theuth web'))).toBe('/tasks/graph?project=theuth%20web')
    expect(viewCardPath(' /tasks ')).toBe('/tasks')
    expect(viewCardPath('/agents/ada')).toBe('/agents/ada')
  })

  it('refuses what is not an app view, and a whiteboard inside a card', () => {
    for (const bad of ['/whiteboards/01J0ABC', '/c/abc', 'https://x.y/tasks', 'tasks', '/tasks/../c/x', '/nope', '', 42]) {
      expect(viewCardPath(bad)).toBeNull()
    }
  })

  it('titles the task graph with its project', () => {
    expect(viewCardTitle(taskGraphPath('theuth'))).toBe('Task graph · theuth')
    expect(viewCardTitle(taskGraphPath(null))).toBe('Task graph')
    expect(viewCardTitle('/agents')).toBe('Agents')
  })

  it('sends chats to the workspace and whiteboards to a pane', () => {
    expect(leavesCard('/c/s%201?x=1')).toEqual({ kind: 'chat', session: 's 1', href: '/c/s%201?x=1' })
    expect(leavesCard('/new')).toEqual({ kind: 'outer', href: '/new' })
    expect(leavesCard('/whiteboards/01J0')).toEqual({ kind: 'pane', href: '/whiteboards/01J0' })
    expect(leavesCard('/tasks/T1')).toEqual({ kind: 'task', id: 'T1', href: '/tasks/T1' })
    expect(leavesCard('/tasks/T%201?tab=x')).toEqual({ kind: 'task', id: 'T 1', href: '/tasks/T%201?tab=x' })
    expect(leavesCard('/tasks/graph?project=a')).toBeNull()
    expect(leavesCard('/tasks')).toBeNull()
  })
})
