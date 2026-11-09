// Z184-R3: Remove's confirm offers to revoke the link the member came in on, and says why.
import '../settings/test-dom'
import { act, createElement as h } from 'react'
import { createRoot } from 'react-dom/client'
import { expect, test, vi } from 'vitest'
import { RemoveMember } from './ShareDialog'

vi.mock('@/components/tasks/queries', () => ({ useMesh: () => ({ data: undefined }) }))
vi.mock('@/lib/rpc', () => ({ useApi: () => ({}) }))
vi.mock('@/components/editor/DocumentPage', () => ({ Action: () => null }))

const mount = async (link: { id: string; name: string | null } | null) => {
  const host = document.createElement('div')
  document.body.append(host)
  const root = createRoot(host)
  const onConfirm = vi.fn()
  await act(async () => root.render(h(RemoveMember, { name: 'Ana', link, onConfirm, onCancel: () => undefined })))
  const button = (text: string) => [...host.querySelectorAll('button')].find((b) => b.textContent === text)
  return { host, root, onConfirm, button }
}

test('with a live link: revoking it is offered, on by default, and the copy says the link still admits others', async () => {
  const { host, root, onConfirm, button } = await mount({ id: 'lnk_1', name: 'team' })
  expect(host.textContent).toContain('Without revoking link «team», anyone else holding it can still join.')
  const box = host.querySelector<HTMLInputElement>('input[type=checkbox]')
  expect(box?.checked).toBe(true)
  await act(async () => button('Remove Ana')?.click())
  expect(onConfirm).toHaveBeenLastCalledWith(true)
  await act(async () => box?.click())
  await act(async () => button('Remove Ana')?.click())
  expect(onConfirm).toHaveBeenLastCalledWith(false)
  await act(async () => root.unmount())
})

test('no live link: nothing to revoke', async () => {
  const { host, root, onConfirm, button } = await mount(null)
  expect(host.querySelector('input[type=checkbox]')).toBeNull()
  await act(async () => button('Remove Ana')?.click())
  expect(onConfirm).toHaveBeenLastCalledWith(false)
  await act(async () => root.unmount())
})
