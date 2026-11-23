import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import { DiffSections, diffFiles } from './DiffBody'

const patch = [
  'diff --git a/src/a.rs b/src/a.rs',
  'index 1111111..2222222 100644',
  '--- a/src/a.rs',
  '+++ b/src/a.rs',
  '@@ -1,2 +1,2 @@',
  ' keep',
  '-old',
  '+new',
  'diff --git a/new.txt b/new.txt',
  'new file mode 100644',
  '--- /dev/null',
  '+++ b/new.txt',
  '@@ -0,0 +1 @@',
  '+hello',
  'diff --git a/logo.png b/logo.png',
  'Binary files a/logo.png and b/logo.png differ',
  '',
].join('\n')
const view = {
  text: patch,
  files: [
    { path: 'src/a.rs', status: 'M', added: 1, removed: 1 },
    { path: 'new.txt', status: 'A', added: 1, removed: 0 },
    { path: 'logo.png', status: 'M', added: 0, removed: 0 },
  ],
}

test('each file of the patch is a section in patch order; a counted file with no text hunks still gets one', () => {
  const { files, stats } = diffFiles(view)
  expect(files.map((f) => f.path)).toEqual(['src/a.rs', 'new.txt', 'logo.png'])
  expect(files[0]!.rows.filter((r) => r.kind === 'line').map((r) => `${r.op}${r.text}`)).toEqual([' keep', '-old', '+new'])
  expect(files[1]!.rows.at(-1)).toMatchObject({ op: '+', text: 'hello', next: 1 })
  expect(files[2]!.rows).toEqual([])
  expect(stats.get('new.txt')).toMatchObject({ status: 'A', added: 1 })
})

test('with stats a section header is a fold button carrying the status and +N −M; the edit-card header keeps hunk counts', () => {
  const { files, stats } = diffFiles(view)
  const tile = renderToStaticMarkup(createElement(DiffSections, { files, stats }))
  expect(tile.match(/aria-expanded="true"/g)).toHaveLength(3)
  expect(tile).toContain('>A</span>')
  expect(tile).toContain('No text changes')
  expect(tile).not.toContain('hunk')
  const card = renderToStaticMarkup(createElement(DiffSections, { files }))
  expect(card).not.toContain('aria-expanded')
  expect(card).toContain('1 hunk')
})

test('the row cap spans files and offers Show all only with a handler', () => {
  const { files } = diffFiles(view)
  const capped = renderToStaticMarkup(createElement(DiffSections, { files, cap: 2, onShowAll: () => {} }))
  expect(capped.match(/<tr/g)).toHaveLength(2)
  expect(capped).not.toContain('new.txt')
  expect(capped).toContain('Show all 6 lines')
  expect(renderToStaticMarkup(createElement(DiffSections, { files, cap: 2 }))).not.toContain('Show all')
})
