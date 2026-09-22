import { readFileSync } from 'node:fs'
import { Folder, Rocket } from 'lucide-react'
import { expect, test } from 'vitest'
import { COLORS, ICONS, colorFor, iconFor } from './project-style'

test('allow-listed keys resolve; unknown or unset keys fall back to the neutral glyph and colour', () => {
  expect(iconFor('Rocket')).toBe(Rocket)
  expect(iconFor('Skull')).toBe(Folder)
  expect(iconFor(null)).toBe(Folder)
  expect(colorFor('teal')).toBe(COLORS.teal)
  expect(colorFor('#ff0000')).toBe('text-fg-tertiary')
  expect(colorFor(undefined)).toBe('text-fg-tertiary')
})

test('the key sets are the server allow-list (crates/agent/src/project.rs ICONS / COLORS)', () => {
  const rs = readFileSync(new URL('../../../../agent/src/project.rs', import.meta.url), 'utf8')
  const list = (name: string) => [...rs.match(new RegExp(`pub const ${name}: &\\[&str\\] = &\\[([^\\]]*)\\]`))![1]!.matchAll(/"([^"]+)"/g)].map((m) => m[1])
  expect(Object.keys(ICONS)).toEqual(list('ICONS'))
  expect(Object.keys(COLORS)).toEqual(list('COLORS'))
})
