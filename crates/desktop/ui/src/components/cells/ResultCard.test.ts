import { createElement as h } from 'react'
import { expect, test } from 'vitest'
import { classify } from '@/lib/injected'
import { ResultCard } from './ResultCard'
import { SESSION, render } from './test-fixtures'

test('<cell_result run=… name=…> is one cell_result card (§9.1, L2)', () => {
  const text = `<cell_result run="r_01JC7Q2M8V" name="preview-ir-prod1" exit="0" secs="41" cwd="~/dr-t6/frp-integration/stacks/sa-frp-integration" interpreter="bash 5.2 (Homebrew)" expect="met 1/1" edited="false" pressed="desktop · Sky&apos;s MacBook" at="14:31">
expect ✓ 3 to update
summary ~3 update · 0 replace · 0 delete
</cell_result>`
  expect(classify(text)).toEqual([
    { kind: 'cell_result', run: 'r_01JC7Q2M8V', name: 'preview-ir-prod1', exit: '0', secs: '41', text: 'expect ✓ 3 to update\nsummary ~3 update · 0 replace · 0 delete' },
  ])
})

test('an unnamed cell carries cell="<ordinal>" message="<entry>"', () => {
  const [card] = classify('<cell_result run="r_2" cell="2" message="42" exit="1" secs="3">\noutput kept from theuth (private)\n</cell_result>') ?? []
  expect(card).toEqual({ kind: 'cell_result', run: 'r_2', cell: '2', message: '42', exit: '1', secs: '3', text: 'output kept from theuth (private)' })
})

test('<cell_error> renders nothing, like <surface_error> (§9.2)', () => {
  expect(classify('<cell_error>\nYour reply presents 1 user cell the user cannot run.\n- cell 3 "apply-ir-prod1": after_unknown\n</cell_error>')).toEqual([])
})

test('the card is one quiet line: theuth read · <name> · exit · secs', () => {
  const html = render(h(ResultCard, { run: 'r_1', session: SESSION, name: 'preview-ir-prod1', exit: '0', secs: '41' }))
  expect(html).toContain('theuth read · preview-ir-prod1 · exit 0 · 41 s')
  expect(render(h(ResultCard, { run: 'r_2', session: SESSION, cell: '2', exit: 'lost', secs: '' }))).toContain('theuth read · cell 2 · lost')
})
