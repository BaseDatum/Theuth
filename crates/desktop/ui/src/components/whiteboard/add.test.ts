import { describe, expect, it } from 'vitest'
import { EXCERPT_LINES, excerptOf, parseFileRef } from './add'

describe('file excerpt', () => {
  it('parses path forms', () => {
    expect(parseFileRef(' src/a.ts ')).toEqual({ path: 'src/a.ts' })
    expect(parseFileRef('src/a.ts:12')).toEqual({ path: 'src/a.ts', start: 12, end: 12 })
    expect(parseFileRef('src/a.ts:12-40')).toEqual({ path: 'src/a.ts', start: 12, end: 40 })
    expect(parseFileRef('src/a.ts#L12-L40')).toEqual({ path: 'src/a.ts', start: 12, end: 40 })
    expect(parseFileRef('C:/x/a.ts')).toEqual({ path: 'C:/x/a.ts' })
    expect(parseFileRef('')).toBeNull()
  })
  it('takes the range, else the first lines, clamped to the file', () => {
    const text = Array.from({ length: 200 }, (_, i) => `line ${i + 1}`).join('\n')
    expect(excerptOf('a.ts', text, 'ts', 10, 12)).toEqual({ t: 'excerpt', path: 'a.ts', start: 10, end: 12, text: 'line 10\nline 11\nline 12', lang: 'ts' })
    const whole = excerptOf('a.ts', text, undefined)
    expect([whole.start, whole.end]).toEqual([1, EXCERPT_LINES])
    expect(excerptOf('a.ts', text, undefined, 190, 999).end).toBe(200)
  })
  it('caps the text at 16 000 chars and says where it ends', () => {
    const text = Array.from({ length: 100 }, () => 'x'.repeat(999)).join('\n')
    const e = excerptOf('big.txt', text, undefined, 1, 100)
    expect(e.text.length).toBe(16_000)
    expect(e.end).toBe(17)
  })
})

describe('webUrl (218 review B1)', () => {
  it('passes absolute http(s) URLs with a dotted host', async () => {
    const { webUrl } = await import('./add')
    expect(webUrl('https://example.com/a?b=1')).toBe('https://example.com/a?b=1')
    expect(webUrl(' http://news.ycombinator.com ')).toBe('http://news.ycombinator.com/')
  })
  it('refuses every other scheme, app origins, bare hosts and credentials', async () => {
    const { webUrl } = await import('./add')
    for (const bad of ['asset://localhost/x', 'tauri://localhost/', 'theuth-page://x/y', 'file:///etc/passwd', 'javascript:alert(1)', 'data:text/html,hi', 'http://localhost:1420/', 'https://user:pw@example.com', 'example.com', '', 42, null])
      expect(webUrl(bad), String(bad)).toBeNull()
  })
})
