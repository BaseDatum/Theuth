// §8.6 colours (A1): named inks are theme tokens; a free `#rrggbb` keeps its hue with lightness moved until it
// reaches 3 : 1 against the canvas.
import type { InkColour } from '../canvas/model'

export const INK_COLOURS = {
  ink: ['#1a1612', '#e8dcc8'],
  red: ['#d9480f', '#ff7a45'],
  blue: ['#1f6feb', '#6ea8ff'],
  // Light green and amber are darkened from the contract's #2f9e44 (2.54 : 1) and #b7791f (2.69 : 1), same hue, to keep
  // ≥ 3 : 1 on bone (design A1); `readable()` of the contract values gives exactly these.
  green: ['#2a8e3d', '#5fd27a'],
  amber: ['#aa701d', '#f2c14e'],
  violet: ['#7048e8', '#a78bfa'],
} as const satisfies Record<string, readonly [light: string, dark: string]>

/** `--bg-page` in `styles.css` (bone / soot): the surface ink is read against. */
export const CANVAS_BG = { light: '#e8dcc8', dark: '#12100e' } as const
export const MIN_CONTRAST = 3

type Rgb = [number, number, number]

export const hexRgb = (hex: string): Rgb => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16)) as Rgb
const rgbHex = (c: Rgb) => `#${c.map((v) => Math.round(Math.min(255, Math.max(0, v))).toString(16).padStart(2, '0')).join('')}`

/** WCAG 2 relative luminance. */
function luminance([r, g, b]: Rgb): number {
  const lin = (v: number) => {
    const s = v / 255
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4
  }
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

export function contrast(a: string, b: string): number {
  const la = luminance(hexRgb(a))
  const lb = luminance(hexRgb(b))
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05)
}

function toHsl([r, g, b]: Rgb): [number, number, number] {
  const [R, G, B] = [r / 255, g / 255, b / 255]
  const max = Math.max(R, G, B)
  const min = Math.min(R, G, B)
  const l = (max + min) / 2
  if (max === min) return [0, 0, l]
  const d = max - min
  const s = l > 0.5 ? d / (2 - max - min) : d / (max + min)
  const h = max === R ? (G - B) / d + (G < B ? 6 : 0) : max === G ? (B - R) / d + 2 : (R - G) / d + 4
  return [h / 6, s, l]
}

function fromHsl(h: number, s: number, l: number): Rgb {
  if (s === 0) return [l * 255, l * 255, l * 255]
  const q = l < 0.5 ? l * (1 + s) : l + s - l * s
  const p = 2 * l - q
  const hue = (t: number) => {
    const u = t < 0 ? t + 1 : t > 1 ? t - 1 : t
    if (u < 1 / 6) return p + (q - p) * 6 * u
    if (u < 1 / 2) return q
    if (u < 2 / 3) return p + (q - p) * (2 / 3 - u) * 6
    return p
  }
  return [hue(h + 1 / 3) * 255, hue(h) * 255, hue(h - 1 / 3) * 255]
}

/** A free colour adjusted for the theme: same hue and saturation, lightness moved away from the canvas until ≥ 3 : 1. */
export function readable(hex: string, dark: boolean): string {
  const bg = dark ? CANVAS_BG.dark : CANVAS_BG.light
  if (contrast(hex, bg) >= MIN_CONTRAST) return hex.toLowerCase()
  const [h, s, l0] = toHsl(hexRgb(hex))
  for (let i = 1; i <= 100; i++) {
    const l = dark ? Math.min(1, l0 + i / 100) : Math.max(0, l0 - i / 100)
    const c = rgbHex(fromHsl(h, s, l))
    if (contrast(c, bg) >= MIN_CONTRAST) return c
  }
  return dark ? '#ffffff' : '#000000'
}

/** The CSS colour of an ink in the current theme. */
export function inkCss(colour: InkColour, dark: boolean): string {
  if (colour in INK_COLOURS) return INK_COLOURS[colour as keyof typeof INK_COLOURS][dark ? 1 : 0]
  return /^#[0-9a-fA-F]{6}$/.test(colour) ? readable(colour, dark) : INK_COLOURS.ink[dark ? 1 : 0]
}

/** The nearest named token for a picked `#rrggbb` (native picker), else the hex itself. */
export function nearestInk(hex: string): InkColour {
  if (!/^#[0-9a-fA-F]{6}$/.test(hex)) return 'ink'
  const [r, g, b] = hexRgb(hex)
  let best: InkColour | null = null
  let dist = 40 // only snap when visibly the same colour
  for (const [name, pair] of Object.entries(INK_COLOURS)) {
    for (const ref of pair) {
      const [R, G, B] = hexRgb(ref)
      const d = Math.hypot(r - R, g - G, b - B)
      if (d < dist) {
        dist = d
        best = name as InkColour
      }
    }
  }
  return best ?? (hex.toLowerCase() as InkColour)
}
