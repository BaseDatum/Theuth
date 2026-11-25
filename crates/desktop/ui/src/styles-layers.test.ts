// iOS WebKit paint budget: no permanent full-pane compositing layers (the iPad left buttons, typed text and the
// right edge of a pane unpainted with two panes open).
import { readFileSync } from 'node:fs'
import { expect, test } from 'vitest'

const css = readFileSync(new URL('./styles.css', import.meta.url), 'utf8')
const trellis = readFileSync(new URL('../node_modules/@danfessler/trellis/dist/style.css', import.meta.url), 'utf8')
const PARTS = ":is([data-trellis-part='panel'], [data-trellis-part='group'], [data-trellis-part='surface'])"

test("Trellis's panes, groups and surfaces drop their permanent will-change at rest, and take it back only while moving", () => {
  // Trellis still ships it on all three (zero-specificity `:where` rules, so ours win).
  for (const part of ['panel', 'group', 'surface']) expect(trellis).toMatch(new RegExp(`:where\\(\\.trellis \\[data-trellis-part="${part}"\\]\\) \\{[^}]*will-change: transform`))
  expect(css).toContain(`.trellis ${PARTS} {\n  will-change: auto;\n}`)
  expect(css).toContain(`.trellis:is([data-dragging], [data-resizing], [data-gesture]) ${PARTS} {\n  will-change: transform;\n}`)
})
