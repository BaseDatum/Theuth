// 194 D5: every model picker passes `effort` (the chip row under the selected model). A source scan, so a new picker
// that forgets it fails here, naming file:line.
import { readdirSync, readFileSync } from 'node:fs'
import { join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'
import { expect, test } from 'vitest'

const SRC = fileURLToPath(new URL('../../', import.meta.url))

/** Wrappers that forward their own `effort` prop to the picker inside them (`file:element`). */
const EXEMPT: Record<string, true> = { 'components/chat/ModelCombobox.tsx:ModelPicker': true }

const sources = (dir: string): string[] =>
  readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const path = join(dir, e.name)
    if (e.isDirectory()) return e.name === 'node_modules' ? [] : sources(path)
    return e.name.endsWith('.tsx') && !/\.test\.tsx$/.test(e.name) ? [path] : []
  })

/** The element's attribute text: from after its name to its closing `>` / `/>`, skipping `{…}` and quoted strings. */
function elementProps(text: string, from: number): string {
  let depth = 0
  for (let i = from; i < text.length; i++) {
    const c = text[i]!
    if (c === '"' || c === "'" || c === '`') {
      const end = text.indexOf(c, i + 1)
      if (end < 0) break
      i = end
    } else if (c === '{') depth++
    else if (c === '}') depth--
    else if (c === '>' && depth === 0) return text.slice(from, i)
  }
  return text.slice(from)
}

/** `file:line` of every `<ModelCombobox` / `<ModelPicker` element in `text` whose props lack `effort=`. */
function missingEffort(file: string, text: string): string[] {
  const out: string[] = []
  for (const match of text.matchAll(/<(ModelCombobox|ModelPicker)\b/g)) {
    const at = match.index!
    const lineStart = text.lastIndexOf('\n', at) + 1
    const before = text.slice(lineStart, at)
    // Prose about the pickers (`//`, `/* */`, JSDoc lines), not JSX.
    if (before.includes('//') || before.includes('/*') || before.trimStart().startsWith('*')) continue
    if (EXEMPT[`${file}:${match[1]}`]) continue
    if (!/(^|\s)effort=/.test(elementProps(text, at + match[0].length))) out.push(`${file}:${text.slice(0, at).split('\n').length}`)
  }
  return out
}

test('every <ModelCombobox> and <ModelPicker> passes effort= (194 D5)', () => {
  const files = sources(SRC)
  const hits = files.flatMap((path) => {
    const text = readFileSync(path, 'utf8')
    return /<Model(Combobox|Picker)\b/.test(text) ? [[relative(SRC, path), text] as const] : []
  })
  expect(hits.length).toBeGreaterThan(10)
  expect(hits.flatMap(([file, text]) => missingEffort(file, text))).toEqual([])
})

test('the scan reads multi-line elements to their own closing bracket, not an arrow or a nested element', () => {
  const src = ['<ModelCombobox', '  value={v}', '  onChange={(id) => set(id > 1 ? <b /> : null)}', '/>', '<ModelPicker models={m} effort={e} />'].join('\n')
  expect(missingEffort('x.tsx', src)).toEqual(['x.tsx:1'])
  expect(missingEffort('x.tsx', '// renders <ModelPicker> without it\n<ModelCombobox effort={undefined} />')).toEqual([])
})
