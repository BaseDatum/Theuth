import '../settings/test-dom'
import { act, createElement as h } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, expect, test, vi } from 'vitest'
import { SidebarProvider } from '@/components/ui/sidebar'
import { ProjectHeader } from './SessionList'

const navigate = vi.fn()
const openNewChat = vi.fn()
vi.mock('@tanstack/react-router', async (original) => ({ ...(await original<object>()), useNavigate: () => navigate }))
vi.mock('@/components/chat/NewChatPane', () => ({ openNewChat: (...a: unknown[]) => openNewChat(...a) }))
vi.mock('./ProjectStyle', () => ({ ProjectIcon: () => null, ProjectStyleDialog: () => null }))

afterEach(() => {
  document.body.innerHTML = ''
  openNewChat.mockClear()
})

test.each([
  ['alpha', 'New chat in “alpha”'],
  [null, 'New chat (no project)'],
])('project %s: the plus sits left of the … and opens a new chat in that project without collapsing', async (project, label) => {
  const toggle = vi.fn()
  const host = document.body.appendChild(document.createElement('div'))
  await act(async () => createRoot(host).render(h(SidebarProvider, null, h(ProjectHeader, { project, collapsed: false, toggle }))))
  const plus = host.querySelector<HTMLButtonElement>(`[aria-label="${label}"]`)!
  const menu = host.querySelector<HTMLButtonElement>('[aria-label="Project options"]')!
  expect(plus.title).toBe(label)
  expect(plus.compareDocumentPosition(menu) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  await act(async () => plus.click())
  expect(openNewChat).toHaveBeenCalledExactlyOnceWith(navigate, project)
  expect(toggle).not.toHaveBeenCalled()
  expect(host.querySelector('[role="menu"]')).toBeNull()
})
