import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { afterEach, expect, test, vi } from 'vitest'
import { PopoutTerminalButton } from './TerminalViewer'

afterEach(() => vi.unstubAllGlobals())

test('the terminal toolbar shows "Open in new window" only where a second window exists (canPopout)', () => {
  const button = () => renderToStaticMarkup(createElement(PopoutTerminalButton, { session: 's1', job: 'j1' }))
  const mac = 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)'
  vi.stubGlobal('window', { __TAURI_INTERNALS__: {}, innerWidth: 1280 })
  vi.stubGlobal('navigator', { userAgent: mac, maxTouchPoints: 0 })
  expect(button()).toContain('aria-label="Open in new window"')
  // The iOS shell: one webview, no window manager.
  vi.stubGlobal('navigator', { userAgent: 'Mozilla/5.0 (iPhone; CPU iPhone OS 17_4 like Mac OS X) AppleWebKit/605.1.15 Mobile/15E148', maxTouchPoints: 5 })
  expect(button()).toBe('')
})
