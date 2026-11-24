import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { MermaidFence, svgCache, svgKey } from './Mermaid'

vi.mock('mermaid', () => ({ default: { initialize: vi.fn(), render: vi.fn() } }))
vi.mock('@/lib/notify', () => ({ notify: Object.assign(vi.fn(), { message: vi.fn(), error: vi.fn() }) }))
vi.mock('@/lib/rpc', () => ({ useApi: () => ({}) }))
vi.mock('@/components/ui/tooltip', () => ({ Tooltip: ({ children }: { children: unknown }) => children, TooltipTrigger: ({ children }: { children: unknown }) => children, TooltipContent: () => null }))
vi.mock('@/lib/theme', () => ({ subscribeDark: () => () => {} }))
vi.mock('@/lib/diagram-theme', () => ({ diagramTokens: () => ({ font: 'sans-serif' }) }))

const chart = 'graph TD\nA-->B'
const render = (code: string) => renderToStaticMarkup(createElement(MermaidFence, { code, isIncomplete: false, language: 'mermaid' }))

test('a chart never rendered shows the placeholder; a cached chart paints its svg on the first render', () => {
  expect(render(chart)).toContain('Rendering diagram…')
  // What the first mount's render queue stores once mermaid returns (135 §2: the second mount must not wait for it).
  svgCache.set(svgKey(chart), '<svg data-cached="1"></svg>')
  const html = render(chart)
  expect(html).toContain('<svg data-cached="1"></svg>')
  expect(html).not.toContain('Rendering diagram…')
  // Different source, different key.
  expect(render(`${chart}\nB-->C`)).toContain('Rendering diagram…')
})
