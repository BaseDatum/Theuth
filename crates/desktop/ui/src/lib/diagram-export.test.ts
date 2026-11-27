import { expect, test } from 'vitest'
import { standalone, viewBox } from './diagram-export'

const MERMAID =
  '<svg aria-roledescription="flowchart-v2" role="graphics-document document" viewBox="0 0 314.5 87.2" style="max-width: 314.5px;" class="flowchart" xmlns="http://www.w3.org/2000/svg" width="100%" id="mermaid-1"><style>#mermaid-1{font-family:monospace}</style><g/></svg>'

test('standalone sizes the root from the viewBox, drops the percent width, and paints a background first', () => {
  const out = standalone(MERMAID, '#fff')
  expect(out).toMatchObject({ width: 315, height: 88 })
  expect(out.svg).toMatch(/^<svg [^>]*width="315" height="88"><rect x="-1" y="-1" width="316.5" height="89.2" fill="#fff"\/><style>/)
  expect(out.svg).not.toContain('100%" id')
  expect(out.svg).not.toContain('max-width')
  expect(out.svg.match(/xmlns="http:\/\/www.w3.org\/2000\/svg"/g)).toHaveLength(1)
})

test('standalone paints the caller-supplied palette background for dark and light diagrams', () => {
  const dark = standalone(MERMAID, '#12100e')
  expect(dark.svg).toMatch(/<rect x="-1" y="-1" width="316.5" height="89.2" fill="#12100e"\/>/)
  expect(dark.svg).not.toContain('fill="#fff"')

  const light = standalone(MERMAID, '#e8dcc8')
  expect(light.svg).toMatch(/<rect x="-1" y="-1" width="316.5" height="89.2" fill="#e8dcc8"\/>/)
  expect(light.svg).not.toContain('fill="#fff"')
})

test('standalone adds xmlns, follows a shifted viewBox origin, and refuses an svg without one', () => {
  expect(standalone('<svg viewBox="-50 -10 10 20"></svg>', '#000').svg).toBe(
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="-50 -10 10 20" width="10" height="20"><rect x="-51" y="-11" width="12" height="22" fill="#000"/></svg>',
  )
  expect(() => standalone('<svg width="10"></svg>', '#fff')).toThrow(/viewBox/)
})

test('viewBox rejects malformed or empty boxes', () => {
  expect(viewBox('0 0 100 50')).toEqual({ x: 0, y: 0, width: 100, height: 50 })
  expect(viewBox('-5,0,100.2,50')).toEqual({ x: -5, y: 0, width: 100.2, height: 50 })
  expect(viewBox('0 0 0 50')).toBeNull()
  expect(viewBox(null)).toBeNull()
  expect(viewBox('a b c d')).toBeNull()
})
