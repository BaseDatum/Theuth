import { readdirSync, readFileSync } from 'node:fs'
import { join, relative } from 'node:path'
import { expect, test } from 'vitest'

// iOS WebKit zooms the page when a focused input's font is under 16 px (the "tap the box and everything jumps"
// bug, seen on the composer, then on the ask card's "Type your own…" field). shadcn's Input/Textarea are
// `text-base md:text-sm` for that reason; a caller may shrink them on desktop only (`md:text-sm`, `md:text-xs`),
// never with a bare `text-sm` / `text-xs`. This scans the sources for such overrides on <Input>/<Textarea>
// (same or next line) and names each offender as file:line. `// mobile-zoom: allow` at the end of the line opts out.
const ROOT = new URL('../..', import.meta.url).pathname
const SRC = join(ROOT, 'src')
const TAG = /<(Input|Textarea)\b/
const SMALL = /(?<![\w:-])text-(xs|sm)\b/
const ALLOW = /\/\/ mobile-zoom: allow\s*$/

function* files(dir: string): Generator<string> {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    if (e.name === 'node_modules' || e.name.startsWith('.')) continue
    const p = join(dir, e.name)
    if (e.isDirectory()) yield* files(p)
    else if (/\.tsx$/.test(e.name) && !e.name.endsWith('.test.tsx')) yield p
  }
}

function scan(): string[] {
  const hits: string[] = []
  for (const f of files(SRC)) {
    if (f.endsWith(join('components', 'ui', 'input.tsx')) || f.endsWith(join('components', 'ui', 'textarea.tsx'))) continue
    const lines = readFileSync(f, 'utf8').split('\n')
    lines.forEach((line, i) => {
      if (!TAG.test(line)) return
      const span = line + '\n' + (lines[i + 1] ?? '')
      if (ALLOW.test(line) || ALLOW.test(lines[i + 1] ?? '')) return
      if (SMALL.test(span)) hits.push(`${relative(ROOT, f)}:${i + 1}`)
    })
  }
  return hits
}

test('no <Input>/<Textarea> shrinks its font below 16 px on mobile (iOS focus zoom)', () => {
  expect(scan()).toEqual([])
})
