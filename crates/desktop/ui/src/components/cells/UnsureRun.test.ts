// 184 §12.3: the inline unsure-author confirm moves focus to Cancel (never Run), Escape cancels, and focus returns on close.
import '../settings/test-dom'
import { act, createElement as h } from 'react'
import { createRoot } from 'react-dom/client'
import { expect, test, vi } from 'vitest'
import { UnsureRun } from './NoteCell'

test('focus moves to Cancel, never Run; Escape cancels, and focus returns to the opener on close', async () => {
  const opener = document.createElement('button')
  document.body.append(opener)
  opener.focus()
  const host = document.createElement('div')
  document.body.append(host)
  const root = createRoot(host)
  const onCancel = vi.fn()
  await act(async () => root.render(h(UnsureRun, { server: 'the hub', onRun: () => undefined, onCancel })))
  const cancel = [...host.querySelectorAll('button')].find((b) => b.textContent === 'Cancel')
  expect(document.activeElement).toBe(cancel)
  cancel?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
  expect(onCancel).toHaveBeenCalledTimes(1)
  await act(async () => root.unmount())
  expect(document.activeElement).toBe(opener)
})
