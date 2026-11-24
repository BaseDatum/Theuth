// `container()` from the webview's UA (31 §7.7): every Tauri shell, then a plain browser.
import { afterEach, expect, test, vi } from 'vitest'
import { container, deviceName, isDesktop, markContainer } from './platform'

const UA = {
  mac: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)',
  linux: 'Mozilla/5.0 (X11; Linux aarch64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15',
  windows:
    'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36 Edg/130.0.0.0',
  iphone: 'Mozilla/5.0 (iPhone; CPU iPhone OS 17_4 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148',
}

function shell(userAgent: string, maxTouchPoints = 0, tauri = true) {
  vi.stubGlobal('window', tauri ? { __TAURI_INTERNALS__: {} } : {})
  vi.stubGlobal('navigator', { userAgent, maxTouchPoints })
}

afterEach(() => vi.unstubAllGlobals())

test.each([
  ['macOS WKWebView', UA.mac, 0, 'macos', 'Mac'],
  ['WebKitGTK', UA.linux, 0, 'linux', 'Linux'],
  ['WebView2', UA.windows, 0, 'windows', 'Windows'],
  ['WebView2 with touch (touchscreen, RDP touch redirection)', UA.windows, 10, 'windows', 'Windows'],
  ['WebKitGTK with touch', UA.linux, 10, 'linux', 'Linux'],
  ['iPhone', UA.iphone, 5, 'ios', 'iPhone'],
  ['iPad (Macintosh UA, touch)', UA.mac, 5, 'ios', 'iPad'],
] as const)('%s → %s', (_, ua, touchPoints, expected, name) => {
  shell(ua, touchPoints)
  expect(container()).toBe(expected)
  expect(isDesktop()).toBe(expected !== 'ios')
  expect(deviceName()).toBe(name)
})

test('no __TAURI_INTERNALS__ → web, never desktop, whatever the UA says', () => {
  for (const ua of Object.values(UA)) {
    shell(ua, 0, false)
    expect(container()).toBe('web')
    expect(isDesktop()).toBe(false)
  }
  shell(UA.windows, 0, false)
  expect(deviceName()).toBe('Edge')
})

test('markContainer: <html data-container> is "ios" in the iOS shell, which turns on the compact status bar (ios: variant); never elsewhere', () => {
  const run = (ua: string, touchPoints: number, tauri = true) => {
    shell(ua, touchPoints, tauri)
    const documentElement = { dataset: {} as DOMStringMap }
    vi.stubGlobal('document', { documentElement })
    markContainer()
    return documentElement.dataset.container
  }
  expect(run(UA.mac, 5)).toBe('ios')
  expect(run(UA.iphone, 5)).toBe('ios')
  expect(run(UA.mac, 0)).toBe('macos')
  expect(run(UA.windows, 10)).toBe('windows')
  expect(run(UA.linux, 0)).toBe('linux')
  expect(run(UA.mac, 5, false)).toBe('web')
})
