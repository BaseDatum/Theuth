import { afterEach, expect, test, vi } from 'vitest'
import { PAGE_VARIABLES, pageDefaults, pageThemeFor } from './page-theme'

/** T3 Code `packages/shared/src/htmlRender.ts:209-244`, verbatim: a page written for `html_render` renders unchanged. */
const T3 = [
  '--background',
  '--foreground',
  '--muted',
  '--muted-foreground',
  '--card',
  '--card-foreground',
  '--popover',
  '--popover-foreground',
  '--secondary',
  '--secondary-foreground',
  '--border',
  '--input',
  '--ring',
  '--primary',
  '--primary-foreground',
  '--accent',
  '--accent-foreground',
  '--accent-surface',
  '--accent-surface-foreground',
  '--destructive',
  '--destructive-foreground',
  '--destructive-surface',
  '--warning',
  '--warning-foreground',
  '--warning-surface',
  '--success',
  '--success-foreground',
  '--info',
  '--info-foreground',
  '--code-background',
  '--code-foreground',
  '--chart-1',
  '--chart-2',
  '--chart-3',
  '--chart-4',
  '--chart-5',
  '--chart-6',
  '--radius',
  '--font-sans',
  '--font-mono',
]

afterEach(() => vi.unstubAllGlobals())

/** A root whose computed style answers `tokens`, with the `.dark` class tracked so the class-lift trick can be observed. */
function stubRoot(dark: boolean, tokens: (dark: boolean) => Record<string, string>) {
  const classes = new Set(dark ? ['dark'] : [])
  const flips: boolean[] = []
  const root = {
    classList: {
      contains: (c: string) => classes.has(c),
      toggle: (c: string, on: boolean) => {
        on ? classes.add(c) : classes.delete(c)
        flips.push(on)
      },
    },
  }
  vi.stubGlobal('document', { documentElement: root })
  vi.stubGlobal('getComputedStyle', () => ({ getPropertyValue: (n: string) => tokens(classes.has('dark'))[n] ?? '' }))
  return { classes, flips }
}

test('maps_all_forty_names', () => {
  expect(T3).toHaveLength(40)
  expect([...PAGE_VARIABLES]).toEqual(T3)
  for (const appearance of ['light', 'dark'] as const) {
    const vars = pageDefaults(appearance)
    expect(Object.keys(vars)).toEqual(T3)
    for (const v of Object.values(vars)) expect(v).not.toBe('')
  }
  // Every token is resolved from the root's computed style; a missing token falls back to the literal palette.
  stubRoot(true, () => ({ '--background': ' #000001 ' }))
  const theme = pageThemeFor('dark')
  expect(Object.keys(theme.variables)).toEqual(T3)
  expect(theme.variables['--background']).toBe('#000001')
  expect(theme.variables['--chart-6']).toBe(pageDefaults('dark')['--chart-6'])
  expect(theme.variables['--font-mono']).toContain('Menlo')
})

test('accent_is_the_brand_token', () => {
  stubRoot(false, () => ({ '--accent': 'neutral-wash', '--fg-accent': 'egyptian-blue', '--fg-inverse': 'bone' }))
  const t = pageThemeFor('light')
  expect(t.variables['--accent']).toBe('egyptian-blue')
  expect(t.variables['--accent-foreground']).toBe('bone')
  expect(t.variables['--chart-1']).toBe('egyptian-blue')
})

test('pageThemeFor lifts the class for the other appearance and puts it back', () => {
  const { classes, flips } = stubRoot(false, (dark) => ({ '--background': dark ? 'soot' : 'bone' }))
  expect(pageThemeFor('light')).toMatchObject({ appearance: 'light', variables: { '--background': 'bone' } })
  expect(flips).toEqual([])
  expect(pageThemeFor('dark')).toMatchObject({ appearance: 'dark', variables: { '--background': 'soot' } })
  expect(flips).toEqual([true, false])
  expect(classes.has('dark')).toBe(false)
})
