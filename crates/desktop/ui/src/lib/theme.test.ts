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

// PTY-COLOR: Tauri (tauri-utils `inject_nonce_token`, tauri `replace_csp_nonce`) stamps a nonce on every <style> in
// index.html and appends `'nonce-…'` to `style-src`; a nonce there makes browsers ignore `'unsafe-inline'`, so every
// stylesheet the app injects at runtime — xterm's `.xterm-fg-N` palette, its font and cell metrics — is silently
// dropped in the built app. Pre-paint CSS goes in public/theme.css (a <link>), never an inline <style>.
test('index.html has no inline <style> (a Tauri nonce there voids style-src unsafe-inline)', () => {
  // Comments are not elements to Tauri's `style` selector either.
  const html = readFileSync(new URL('../../index.html', import.meta.url), 'utf8').replace(/<!--[\s\S]*?-->/g, '')
  expect(html).not.toMatch(/<style[\s>]/i)
})
