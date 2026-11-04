import { createElement as h, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import { TooltipProvider } from '@/components/ui/tooltip'
import type { CellResultInput } from '@/lib/injected'
import { useToolRuns } from '@/lib/tool-runs'
import { LongText } from './LongText'
import { SentResults } from './SentResults'

const render = (child: ReactNode) => renderToStaticMarkup(h(TooltipProvider, null, child))
const TEXT = Array.from({ length: 30 }, (_, i) => `pasted line ${i}`).join('\n')

test('171F B6: a long text folds to eight lines with the full text in the DOM; open shows Show less', () => {
  useToolRuns.setState({ disclosed: {} })
  const shut = render(h(LongText, { text: TEXT, className: 'b', disclosure: 's|u1|text' }))
  expect(shut).toContain('line-clamp-8')
  expect(shut).toContain('Show all 30 lines')
  expect(shut).toContain(TEXT)
  expect(shut).toContain('aria-label="Copy"')
  useToolRuns.setState({ disclosed: { 's|u1|text': true } })
  const open = render(h(LongText, { text: TEXT, className: 'b', disclosure: 's|u1|text' }))
  expect(open).not.toContain('line-clamp-8')
  expect(open).toContain('Show less')
  expect(open).toContain(TEXT)
  // Short text is the plain bubble.
  expect(render(h(LongText, { text: 'ok done', className: 'b', disclosure: 'x' }))).toBe('<div class="b">ok done</div>')
})

test('171F B4: sent cell results are one collapsed row; open lists one result line each', () => {
  const cells: CellResultInput[] = [
    { kind: 'cell_result', run: 'r_a', name: 'say-hello', label: 'Say hello', exit: '0', secs: '1', text: 'expect ✓ hello' },
    { kind: 'cell_result', run: 'r_b', cell: '2', exit: '1', secs: '3', text: 'secret output' },
  ]
  useToolRuns.setState({ disclosed: {} })
  const shut = render(h(SentResults, { cells, disclosure: 's|u1|cells' }))
  expect(shut).toContain('2 cell results sent with this message')
  expect(shut).not.toContain('theuth read')
  expect(render(h(SentResults, { cells: cells.slice(0, 1), disclosure: 's|u1|cells' }))).toContain('1 cell result sent with this message')
  useToolRuns.setState({ disclosed: { 's|u1|cells': true } })
  const open = render(h(SentResults, { cells, disclosure: 's|u1|cells' }))
  expect(open).toContain('theuth read · Say hello · exit 0 · 1 s')
  expect(open).toContain('theuth read · cell 2 · exit 1 · 3 s')
  expect(open).not.toContain('secret output')
})
