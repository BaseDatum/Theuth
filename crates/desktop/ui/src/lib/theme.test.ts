import { readFileSync } from 'node:fs'
import { expect, test } from 'vitest'

// public/theme.js runs blocking in <head> before first paint (DARK-FLASH). It must pick the same class
// `lib/theme.ts` would: stored `dark`/`light` win, anything else follows `prefers-color-scheme`.
const SCRIPT = readFileSync(new URL('../../public/theme.js', import.meta.url), 'utf8')

function run(stored: string | null, prefersDark: boolean): boolean {
  let dark = false
  new Function(
    'localStorage',
    'matchMedia',
    'document',
    SCRIPT,
  )(
    { getItem: (k: string) => (k === 'theme' ? stored : null) },
    (q: string) => ({ matches: q === '(prefers-color-scheme: dark)' && prefersDark }),
    { documentElement: { classList: { add: (c: string) => (dark ||= c === 'dark') } } },
  )
  return dark
}

test.each([
  ['dark', false, true],
  ['dark', true, true],
  ['light', false, false],
  ['light', true, false],
  ['system', false, false],
  ['system', true, true],
  [null, false, false],
  [null, true, true],
] as const)('stored=%s prefersDark=%s → dark=%s', (stored, prefersDark, dark) => {
  expect(run(stored, prefersDark)).toBe(dark)
})
