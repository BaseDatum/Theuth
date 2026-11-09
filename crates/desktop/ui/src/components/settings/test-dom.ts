// Test-only: a DOM for the tests that mount. Import it FIRST: Radix's Portal decides at import time whether a DOM exists.
// The apps package carries the repo's jsdom.
import { createRequire } from 'node:module'

const { JSDOM } = createRequire(import.meta.url)('../../../../../../packages/apps/node_modules/jsdom')
const { window } = new JSDOM('<!doctype html><body></body>', { pretendToBeVisual: true })
window.matchMedia = () => ({ matches: false, addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {} })
Object.assign(globalThis, { window, self: window, document: window.document, IS_REACT_ACT_ENVIRONMENT: true, ResizeObserver: class { observe() {} unobserve() {} disconnect() {} } })
window.HTMLCanvasElement.prototype.getContext = () => null
// Every global a browser has (own and inherited: `addEventListener` lives on the prototype chain), bound to the window;
// Node's own events give way to jsdom's, whose `dispatchEvent` accepts only those. Writable, so fake timers can replace them.
const events = ['Event', 'CustomEvent', 'UIEvent', 'KeyboardEvent', 'MouseEvent', 'PointerEvent', 'FocusEvent', 'InputEvent']
for (let o = window; o && o !== Object.prototype; o = Object.getPrototypeOf(o))
  for (const k of Object.getOwnPropertyNames(o))
    if (!(k in globalThis) || events.includes(k))
      Object.defineProperty(globalThis, k, {
        configurable: true,
        get: () => (typeof window[k] === 'function' && !/^[A-Z]/.test(k) ? window[k].bind(window) : window[k]),
        set: (value) => Object.defineProperty(globalThis, k, { value, writable: true, configurable: true }),
      })
