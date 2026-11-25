import './test-dom'
import { QueryClient, QueryClientProvider, notifyManager } from '@tanstack/react-query'
import { act, createElement } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import { type PromptSection, SystemPrompt } from './SystemPrompt'

const CORE_STANDING: PromptSection = {
  id: 'core.standing',
  group: 'core',
  label: 'Standing instructions',
  description: 'Standing instructions loaded from prompt.md on every turn.',
  content: 'Always verify changes before reporting.',
  defaultContent: 'Always verify changes before reporting.',
  customized: false,
  enabled: true,
}

const CORE_HEADER: PromptSection = {
  id: 'core.header',
  group: 'core',
  label: 'System persona',
  description: 'Primary persona header at the top of the system prompt.',
  content: 'Custom persona text.',
  defaultContent: 'You are theuth, a meta-harness on the user machine.',
  customized: true,
  enabled: true,
}

const PLUGIN_SURFACE: PromptSection = {
  id: 'plugin.surface',
  group: 'plugin',
  label: 'Show first, then tell',
  description: 'Guidance for the surface plugin.',
  content: 'Render visual output first when helpful.',
  defaultContent: 'Render visual output first when helpful.',
  customized: false,
  enabled: true,
}

const state = vi.hoisted(() => ({
  sections: [] as PromptSection[],
  list: vi.fn<() => Promise<PromptSection[]>>(),
  set: vi.fn<(id: string, content: string) => Promise<PromptSection>>(),
  setEnabled: vi.fn<(id: string, enabled: boolean) => Promise<PromptSection>>(),
  reset: vi.fn<(id: string) => Promise<PromptSection>>(),
}))

vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({
    list_prompt_sections: state.list,
    set_prompt_section: state.set,
    set_prompt_section_enabled: state.setEnabled,
    reset_prompt_section: state.reset,
  }),
}))
vi.mock('@/rpc/capabilities', () => ({ useQuerySupport: () => true }))
vi.mock('@/lib/notify', () => ({
  notify: Object.assign(vi.fn(), { error: vi.fn(), message: vi.fn(), success: vi.fn() }),
}))
vi.mock('@/components/agents/CharterEditor', () => ({
  CharterEditor: ({ value, onChange }: { value: string; onChange?: (v: string) => void }) =>
    createElement('textarea', {
      'aria-label': 'Prompt content',
      value,
      onChange: (e: { target: { value: string } }) => onChange?.(e.target.value),
    }),
}))

notifyManager.setScheduler(queueMicrotask)

let unmount: (() => void) | null = null

beforeEach(() => {
  state.sections = [
    { ...CORE_STANDING },
    { ...CORE_HEADER },
    { ...PLUGIN_SURFACE },
  ]
  state.list.mockReset().mockImplementation(async () => state.sections.map((s) => ({ ...s })))
  state.set.mockReset().mockImplementation(async (id, content) => {
    const found = state.sections.find((s) => s.id === id)!
    const updated: PromptSection = {
      ...found,
      content,
      customized: content !== found.defaultContent,
    }
    state.sections = state.sections.map((s) => (s.id === id ? updated : s))
    return updated
  })
  state.setEnabled.mockReset().mockImplementation(async (id, enabled) => {
    const found = state.sections.find((s) => s.id === id)!
    const updated: PromptSection = { ...found, enabled }
    state.sections = state.sections.map((s) => (s.id === id ? updated : s))
    return updated
  })
  state.reset.mockReset().mockImplementation(async (id) => {
    const found = state.sections.find((s) => s.id === id)!
    const updated: PromptSection = {
      ...found,
      content: found.defaultContent,
      customized: false,
      enabled: true,
    }
    state.sections = state.sections.map((s) => (s.id === id ? updated : s))
    return updated
  })
})

afterEach(() => {
  unmount?.()
  unmount = null
  document.body.innerHTML = ''
})

async function mount() {
  const el = document.createElement('div')
  document.body.append(el)
  const root = createRoot(el)
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  await act(async () =>
    root.render(createElement(QueryClientProvider, { client: qc, children: createElement(SystemPrompt) })),
  )
  await vi.waitFor(() => expect(el.textContent).toContain('Standing instructions'))
  unmount = () => {
    act(() => root.unmount())
    el.remove()
  }

  const buttonByText = (text: string) =>
    [...el.querySelectorAll<HTMLButtonElement>('button')].find(
      (b) => b.textContent?.trim() === text || b.textContent?.includes(text),
    )

  const exactButton = (text: string) =>
    [...el.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent?.trim() === text)

  const click = async (b: HTMLElement | undefined) => {
    expect(b).toBeDefined()
    await act(async () => b!.click())
  }

  const setField = async (field: HTMLInputElement | HTMLTextAreaElement, value: string) => {
    const proto =
      field instanceof window.HTMLTextAreaElement
        ? window.HTMLTextAreaElement.prototype
        : window.HTMLInputElement.prototype
    Object.getOwnPropertyDescriptor(proto, 'value')?.set?.call(field, value)
    await act(async () => field.dispatchEvent(new window.Event('input', { bubbles: true })))
  }

  return { el, qc, buttonByText, exactButton, click, setField }
}

test('renders core and plugin sections grouped into System & core and Plugins', async () => {
  const { el } = await mount()
  const headings = [...el.querySelectorAll('h3')].map((h) => h.textContent?.trim())
  expect(headings).toEqual(['System & core', 'Plugins'])

  const sections = [...el.querySelectorAll('section')]
  expect(sections[0]?.textContent).toContain('Standing instructions')
  expect(sections[0]?.textContent).toContain('core.standing')
  expect(sections[0]?.textContent).toContain('System persona')
  expect(sections[0]?.textContent).toContain('Modified')
  expect(sections[1]?.textContent).toContain('Show first, then tell')
  expect(sections[1]?.textContent).toContain('plugin.surface')
})

test('expands a section, edits text, and calls set_prompt_section on Save', async () => {
  const { el, buttonByText, exactButton, click, setField } = await mount()
  const header = buttonByText('Standing instructions')
  expect(header?.getAttribute('aria-expanded')).toBe('false')

  await click(header)
  expect(header?.getAttribute('aria-expanded')).toBe('true')
  expect(el.textContent).toContain(CORE_STANDING.description)

  const editor = el.querySelector<HTMLTextAreaElement>('textarea[aria-label="Prompt content"]')!
  expect(editor.value).toBe(CORE_STANDING.content)
  expect(exactButton('Save')?.disabled).toBe(true)

  await setField(editor, 'Updated standing instructions.')
  expect(el.textContent).toContain('Unsaved')
  expect(exactButton('Save')?.disabled).toBe(false)

  await click(exactButton('Save'))
  await vi.waitFor(() =>
    expect(state.set).toHaveBeenCalledWith('core.standing', 'Updated standing instructions.'),
  )
  await vi.waitFor(() => expect(el.textContent).not.toContain('Unsaved'))
})

test('calls reset_prompt_section on Reset to default for a customized section', async () => {
  const { el, buttonByText, exactButton, click } = await mount()
  await click(buttonByText('System persona'))

  const resetBtn = exactButton('Reset to default')
  expect(resetBtn?.disabled).toBe(false)

  await click(resetBtn)
  await vi.waitFor(() => expect(state.reset).toHaveBeenCalledWith('core.header'))
  await vi.waitFor(() => {
    const editor = el.querySelector<HTMLTextAreaElement>('textarea[aria-label="Prompt content"]')
    expect(editor?.value).toBe(CORE_HEADER.defaultContent)
  })
})

test('filters by search query and by Modified filter', async () => {
  const { el, exactButton, click, setField } = await mount()
  const search = el.querySelector<HTMLInputElement>('input[aria-label="Filter sections"]')!

  await setField(search, 'surface')
  expect(el.textContent).toContain('Show first, then tell')
  expect(el.textContent).not.toContain('Standing instructions')
  expect([...el.querySelectorAll('h3')].map((h) => h.textContent?.trim())).toEqual(['Plugins'])

  await setField(search, '')
  await click(exactButton('Modified (1)'))
  expect(el.textContent).toContain('System persona')
  expect(el.textContent).not.toContain('Standing instructions')
  expect(el.textContent).not.toContain('Show first, then tell')
})

test('renders per-section switch reflecting enabled state and toggles without expanding the editor', async () => {
  state.sections = [
    { ...CORE_STANDING, enabled: true },
    { ...CORE_HEADER, enabled: true },
    { ...PLUGIN_SURFACE, enabled: false },
  ]
  const { el, buttonByText, click } = await mount()

  const standingSwitch = el.querySelector<HTMLButtonElement>(
    'button[role="switch"][aria-label="Enable Standing instructions"]',
  )
  const headerSwitch = el.querySelector<HTMLButtonElement>(
    'button[role="switch"][aria-label="Enable System persona"]',
  )
  const surfaceSwitch = el.querySelector<HTMLButtonElement>(
    'button[role="switch"][aria-label="Enable Show first, then tell"]',
  )

  expect(standingSwitch?.getAttribute('aria-checked')).toBe('true')
  expect(headerSwitch?.getAttribute('aria-checked')).toBe('true')
  expect(surfaceSwitch?.getAttribute('aria-checked')).toBe('false')

  await click(standingSwitch!)
  await vi.waitFor(() => expect(state.setEnabled).toHaveBeenCalledWith('core.standing', false))
  await vi.waitFor(() => expect(standingSwitch?.getAttribute('aria-checked')).toBe('false'))

  expect(el.querySelector('textarea[aria-label="Prompt content"]')).toBeNull()
  expect(buttonByText('Standing instructions')?.getAttribute('aria-expanded')).toBe('false')
})

test('shows Disabled badge, matches Disabled filter, and enables Reset to default for disabled sections', async () => {
  state.sections = [
    { ...CORE_STANDING, enabled: true },
    { ...CORE_HEADER, enabled: true },
    { ...PLUGIN_SURFACE, enabled: false },
  ]
  const { el, buttonByText, exactButton, click } = await mount()

  expect(el.textContent).toContain('Disabled')

  await click(exactButton('Disabled (1)'))
  expect(el.textContent).toContain('Show first, then tell')
  expect(el.textContent).not.toContain('Standing instructions')
  expect(el.textContent).not.toContain('System persona')

  await click(buttonByText('Show first, then tell'))
  const resetBtn = exactButton('Reset to default')
  expect(resetBtn?.disabled).toBe(false)

  await click(resetBtn)
  await vi.waitFor(() => expect(state.reset).toHaveBeenCalledWith('plugin.surface'))
  await vi.waitFor(() => expect(el.textContent).toContain('Disabled (0)'))
})
