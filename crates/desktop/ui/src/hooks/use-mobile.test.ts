import { createElement } from 'react'
import { renderToString } from 'react-dom/server'
import { afterEach, expect, test, vi } from 'vitest'
import { useIsMobile } from './use-mobile'

const Probe = () => createElement('i', null, useIsMobile() ? 'phone' : 'desktop')

/** A window whose media query and `innerWidth` may disagree: WKWebView after a resume or a rotation (WebKit 170595, 262207). */
function stubWindow(matches: boolean, innerWidth: number): void {
  const mql = { matches, addEventListener: () => undefined, removeEventListener: () => undefined }
  vi.stubGlobal('window', {
    innerWidth,
    matchMedia: (q: string) => {
      // Tailwind's `md:` (the sidebar's `hidden md:block`) negated: JS and CSS must read the same breakpoint.
      expect(q).toBe('(max-width: 767px)')
      return mql
    },
  })
}

afterEach(() => vi.unstubAllGlobals())

test('a phone is mobile on its very first render: a remount never paints a header without the sidebar toggle', () => {
  stubWindow(true, 390)
  expect(renderToString(createElement(Probe))).toContain('phone')
})

test('the media query is the only source: a stale desktop-sized innerWidth cannot turn a phone into a desktop', () => {
  stubWindow(true, 980)
  expect(renderToString(createElement(Probe))).toContain('phone')
  stubWindow(false, 390)
  expect(renderToString(createElement(Probe))).toContain('desktop')
})
