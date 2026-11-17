import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement as h } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { TooltipProvider } from '@/components/ui/tooltip'
import type { ContextEntry, ContextPiece } from '@/rpc/bindings/method.agent.output'
import type * as Rpc from '@/lib/rpc'
import type * as Speech from '@/lib/speech'
import { useToolRuns } from '@/lib/tool-runs'
import { empty } from '@/store/reduce'
import { active } from '@/store/engine-scope'
import { type Entry, type SessionSlice, useSessions } from '@/store/sessions'
import { MessageTool } from './MessageTool'
import { DisclosureScope } from './disclosure'
import { MessageAssistant } from './MessageAssistant'

vi.mock('@tanstack/react-router', () => ({ useNavigate: () => vi.fn(), useRouter: () => undefined }))
vi.mock('@/components/hud/queries', () => ({ useMarkShown: () => {} }))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => ({}) }))
vi.mock('@/lib/speech', async (original) => ({ ...(await original<typeof Speech>()), useVoice: () => false, useSpeaking: () => null }))
vi.mock('@/lib/mobile', () => ({ usePointerCoarse: () => false }))
// Server rendering reads zustand's initial state: the sessions store answers its live state.
Object.assign(active().sessions, { getInitialState: active().sessions.getState })

const S = 's1'
const TEXT = [
  'The engine branch is in.',
  '```mermaid',
  'sequenceDiagram',
  'A->>B: first',
  '```',
  '```rust',
  'fn main() {}',
  '```',
  '```mermaid',
  'graph TD',
  'A-->B',
  '```',
  'Two things need checking.',
].join('\n')
const piece = (index: number, label: string, removed = false): ContextPiece => ({
  target: { kind: 'fence', owner: { kind: 'message', seq: 7 }, part: 0, index },
  label,
  removed,
})
const context = (fences: ContextPiece[], sources?: ContextPiece[]): ContextEntry => ({
  target: { kind: 'message', seq: 7 },
  active: true,
  removed: false,
  remove_with: [],
  attachments: [],
  fences,
  sources,
})
const slice = (over: Partial<SessionSlice> = {}) =>
  useSessions.setState({ sessions: { [S]: { ...empty(S), loaded: true, contextEditable: true, contextRevision: '3', ...over } } })
const render = (fences: ContextPiece[]) =>
  renderToStaticMarkup(
    h(
      QueryClientProvider,
      { client: new QueryClient() },
      h(TooltipProvider, null, h(DisclosureScope, { value: 'e1' }, h(MessageAssistant, { id: 'e1', text: TEXT, open: false, session: S, context: context(fences) as never }))),
    ),
  )
const control = (html: string, label: string) => html.match(new RegExp(`<button[^>]*aria-label="Remove from context: ${label}"[^>]*>`))?.[0]

test('each surface fence gets its own corner control when the stored fences match the fences in the reply', () => {
  slice()
  const html = render([piece(0, 'sequenceDiagram'), piece(1, 'graph')])
  expect(control(html, 'sequenceDiagram')).toBeDefined()
  expect(control(html, 'graph')).toBeDefined()
  expect(control(html, 'sequenceDiagram')).not.toContain('aria-disabled="true"')
  // The rust fence is not a surface fence: one control per diagram, no more.
  expect(html.match(/aria-label="Remove from context: /g)).toHaveLength(2)
})

test('a count mismatch offers no control at all', () => {
  slice()
  expect(render([piece(0, 'sequenceDiagram')])).not.toContain('Remove from context: ')
  expect(render([piece(0, 'a'), piece(1, 'b'), piece(2, 'c')])).not.toContain('Remove from context: ')
})

test('a removed fence collapses to the marker with View original; opening it shows the archived diagram', () => {
  slice()
  const html = render([piece(0, 'sequenceDiagram', true), piece(1, 'graph')])
  expect(html).toContain('Removed from context</span> · sequenceDiagram')
  expect(html).toContain('View original')
  expect(html.match(/Rendering diagram/g)).toHaveLength(1)
  expect(control(html, 'graph')).toBeDefined()
  useToolRuns.getState().setDisclosed('e1|removed-fence|0', true)
  const opened = render([piece(0, 'sequenceDiagram', true), piece(1, 'graph')])
  expect(opened).toContain('Hide original')
  expect(opened.match(/Rendering diagram/g)).toHaveLength(2)
  useToolRuns.getState().setDisclosed('e1|removed-fence|0', false)
})

test('busy, stale and external sessions keep the control but disable it', () => {
  for (const over of [{ running: true }, { contextStale: true }, { contextEditable: false }]) {
    slice(over)
    expect(control(render([piece(0, 'sequenceDiagram'), piece(1, 'graph')]), 'graph')).toContain('aria-disabled="true"')
  }
})

type Tool = Extract<Entry, { kind: 'tool' }>
// §5.1: the card's tool entry carries its own call's source piece (owner: the assistant row that made the call).
const card = (removed: boolean, name = 'show') => {
  const tool: Tool = {
    id: 'e2',
    kind: 'tool',
    callId: 'c1',
    name,
    args: name === 'page_preview' ? { title: 'mock.html', html: '<p>hi</p>' } : { name: 'mock.html', content: '<p>hi</p>' },
    pending: false,
    child: false,
    result: { output: 'shown mock.html', isError: false, files: [] },
    context: context([], [{ target: { kind: 'source', owner: { kind: 'message', seq: 6 }, part: 1 }, label: 'mock.html', removed }]) as never,
  }
  return renderToStaticMarkup(h(TooltipProvider, null, h(MessageTool, { tool, session: S })))
}

test('show, app and page_preview cards offer Remove source from context; once removed the card stays with a small tag', () => {
  slice()
  for (const name of ['show', 'app', 'page_preview']) {
    const live = card(false, name)
    expect(live).toContain('aria-label="Remove source from context"')
    expect(live).toContain('shown mock.html')
    const gone = card(true, name)
    expect(gone).toContain('Source removed from context')
    expect(gone).not.toContain('aria-label="Remove source from context"')
    expect(gone).toContain('shown mock.html')
  }
})
