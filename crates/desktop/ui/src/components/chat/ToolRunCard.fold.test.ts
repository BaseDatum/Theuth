import '../settings/test-dom'
import { act, createElement } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, expect, test, vi } from 'vitest'
import { useAnimations } from '@/lib/animations'
import { useToolRuns } from '@/lib/tool-runs'
import type { Entry } from '@/store/sessions'
import { ToolRunCard } from './ToolRunCard'

vi.mock('@tanstack/react-router', () => ({ Link: ({ children }: { children: unknown }) => children }))

type Tool = Extract<Entry, { kind: 'tool' }>
const tool = (n: number, extra: Partial<Tool> = {}): Tool => ({
  id: `e${n}`,
  kind: 'tool',
  callId: `c${n}`,
  name: `t${n}`,
  args: { n },
  pending: false,
  child: false,
  result: { output: `out ${n}`, isError: false, files: [] },
  ...extra,
})

let unmount = () => {}
afterEach(() => {
  act(() => unmount())
  unmount = () => {}
  document.body.innerHTML = ''
  useToolRuns.setState({ disclosed: {} })
  useAnimations.getState().setEnabled(true)
  vi.useRealTimers()
})

function mountRun(run: Entry[]) {
  const host = document.createElement('div')
  document.body.append(host)
  const root = createRoot(host)
  act(() => root.render(createElement(ToolRunCard, { run, prompts: null, session: 's1' })))
  unmount = () => root.unmount()
  return {
    rerender: (next: Entry[]) => act(() => root.render(createElement(ToolRunCard, { run: next, prompts: null, session: 's1' }))),
  }
}

test('consecutive tool run expands with staggered fan-in, stays mounted through fold-shut exit, and absorbs live joins', () => {
  const rect = vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({
    x: 0, y: 0, width: 640, height: 180, top: 0, left: 0, right: 640, bottom: 180, toJSON: () => ({}),
  })
  try {
    const running = [tool(1), tool(2, { result: undefined })]
    const { rerender } = mountRun(running)
    const header = document.querySelector('.tool-run-fold-in')
    expect(header).not.toBeNull()

    // A third consecutive tool folds into the running stack: header pulses absorb.
    rerender([tool(1), tool(2), tool(3, { result: undefined })])
    expect(header?.className).toContain('tool-run-absorb')

    vi.useFakeTimers()
    const toggle = document.querySelector('button[aria-expanded]') as HTMLButtonElement
    act(() => toggle.click())
    const fold = document.querySelector('[data-tool-fold]') as HTMLDivElement
    expect(fold).not.toBeNull()
    expect(fold.querySelector('.tool-fold-stage')?.className).toContain('tool-fold-in')
    expect(fold.style.height).toBe('180px')

    act(() => { vi.advanceTimersByTime(340) })
    expect(fold.style.height).toBe('')
    expect(fold.querySelector('.tool-fold-stage')?.className).not.toContain('tool-fold-in')

    // Collapse glides shut and stays mounted during exit, then unmounts.
    act(() => toggle.click())
    expect(fold.getAttribute('aria-hidden')).toBe('true')
    expect(fold.querySelector('.tool-fold-stage')?.className).toContain('tool-fold-out')
    expect(fold.style.height).toBe('0px')

    act(() => { vi.advanceTimersByTime(280) })
    expect(document.querySelector('[data-tool-fold]')).toBeNull()
  } finally {
    rect.mockRestore()
  }
})

test('disabling Appearance › Animations makes tool-run expand/collapse instant with zero animation classes', () => {
  const rect = vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({
    x: 0, y: 0, width: 640, height: 180, top: 0, left: 0, right: 640, bottom: 180, toJSON: () => ({}),
  })
  try {
    useAnimations.getState().setEnabled(false)
    mountRun([tool(1), tool(2, { result: undefined })])
    expect(document.querySelector('.tool-run-fold-in')).toBeNull()

    const toggle = document.querySelector('button[aria-expanded]') as HTMLButtonElement
    act(() => toggle.click())
    const fold = document.querySelector('[data-tool-fold]') as HTMLDivElement
    expect(fold).not.toBeNull()
    expect(fold.querySelector('.tool-fold-stage')?.className).not.toContain('tool-fold-in')
    expect(fold.style.height).toBe('')

    act(() => toggle.click())
    expect(document.querySelector('[data-tool-fold]')).toBeNull()
  } finally {
    rect.mockRestore()
  }
})
