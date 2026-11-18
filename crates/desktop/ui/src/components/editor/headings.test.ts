import '@/components/settings/test-dom'
import { readFileSync } from 'node:fs'
import { BlockNoteEditor } from '@blocknote/core'
import { schema } from '@theuth/documents/schema'
import { expect, test } from 'vitest'

// The app's page sheet over the real editor DOM. jsdom cascades by source order, not specificity, so BlockNote's own
// sheet stays out: `styles.css` must carry the whole scale itself (its unlayered base rule outranks BlockNote's levels).
// jsdom cannot parse the Tailwind source as a whole, so it gets the top-level rules one by one and skips what it rejects.
const sheet = document.head.appendChild(document.createElement('style')).sheet!
const css = readFileSync(new URL('../../styles.css', import.meta.url), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '')
for (let i = 0, depth = 0, start = 0; i < css.length; i++) {
  if (css[i] === '{') depth++
  else if (css[i] === '}' && --depth === 0) {
    try {
      sheet.insertRule(css.slice(start, i + 1).replace(/^(\s*@[^{;]*;)+/, ''), sheet.cssRules.length)
    } catch {}
    start = i + 1
  }
}

test('every heading level renders at its own size, re-rendered on a level change (Streamdown 30/24/20/18/16/14)', () => {
  const editor = BlockNoteEditor.create({ schema, initialContent: [{ type: 'heading', props: { level: 1 }, content: 'Title' }] })
  const root = document.body.appendChild(document.createElement('div'))
  editor.mount(root)
  const id = editor.document[0]!.id
  const sizes = [1, 2, 3, 4, 5, 6].map((level) => {
    editor.updateBlock(id, { props: { level: level as 1 } })
    expect(editor.document[0]!.props).toMatchObject({ level })
    return getComputedStyle(root.querySelector(`[data-id="${id}"] [data-content-type="heading"]`)!).getPropertyValue('--level').trim()
  })
  expect(sizes).toEqual(['30px', '24px', '20px', '18px', '16px', '14px'])
  // BlockNote starts its font-size transition at `--prev-level` (set on the block's outer, one frame): the same scale,
  // not its own 3em..0.8em.
  editor.updateBlock(id, { props: { level: 1 } })
  expect(getComputedStyle(root.querySelector('[data-prev-level="6"]')!).getPropertyValue('--prev-level').trim()).toBe('14px')
  editor.unmount()
})
