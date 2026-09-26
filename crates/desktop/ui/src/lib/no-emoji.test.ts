import { readdirSync, readFileSync } from 'node:fs'
import { join, relative } from 'node:path'
import { expect, test } from 'vitest'

// The UI carries no emoji: every glyph is a lucide icon (rule: "there should be no emojis in the UI"). This scans
// the desktop sources and the apps library for emoji code points and names each offender as file:line.
// A line that legitimately needs one (an emoji picker, a width test) ends with `// no-emoji: allow`.
const ROOT = new URL('../..', import.meta.url).pathname
const ROOTS = ['src', '../../../packages/apps/src', '../../../packages/apps/generated', '../../../packages/apps/test']
const EXT = /\.(tsx?|jsx?|css|html|json|md)$/
// Extended_Pictographic covers the text-default symbols (⚠ ⚙ ✖ ⏸) that fonts still draw as emoji; the arrows block // no-emoji: allow
// (↔ ↩, U+2190–21FF) and © ® ™ are typography, not emoji, so they pass.
const EMOJI = /\p{Extended_Pictographic}|\p{Emoji_Presentation}|\p{Regional_Indicator}|[0-9#*]\uFE0F?\u20E3|\uFE0F/gu
const TEXT = /^[\u2190-\u21FF\u00A9\u00AE\u2122]$/
const ALLOW = /\/\/ no-emoji: allow\s*$/

function* files(dir: string): Generator<string> {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    if (e.name === 'node_modules' || e.name.startsWith('.')) continue
    const p = join(dir, e.name)
    if (e.isDirectory()) yield* files(p)
    else if (EXT.test(e.name)) yield p
  }
}

function scan(): string[] {
  const hits: string[] = []
  for (const root of ROOTS) {
    for (const f of files(join(ROOT, root))) {
      readFileSync(f, 'utf8')
        .split('\n')
        .forEach((line, i) => {
          if (ALLOW.test(line)) return
          const found = [...new Set(line.match(EMOJI) ?? [])].filter((g) => !TEXT.test(g))
          if (found.length) hits.push(`${relative(ROOT, f)}:${i + 1}  ${found.join(' ')}`)
        })
    }
  }
  return hits
}

// ~300 ms alone; a cold parallel run on a busy machine has taken 12 s.
test('no emoji in the desktop UI or the apps library', () => {
  const hits = scan()
  expect(hits, `${hits.length} emoji line(s):\n${hits.join('\n')}`).toEqual([])
}, 60_000)
