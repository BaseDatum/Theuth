// 59 §10 / §14 row 6, automated proxy for the physical-iPhone check (step 6 as-built): headless Chromium
// emulating an iPhone 14 (390×844 @3, touch, `pointer: coarse`) on the Vite UI against a headless harness,
// plus a desktop-emulation page on the same note. Puppeteer cannot shrink `visualViewport`, so the keyboard
// is simulated: `window.__theuthKbInset = 336` (honoured by `useKeyboardInset`) and `--visible-band: 508px
// !important` (what `lib/viewport.ts` would publish on a real device).
//
//   bun scripts/mobile-smoke.ts --ui http://127.0.0.1:1451 --rpc ws://127.0.0.1:7821 --token $(cat $DATA/token) \
//       [--out /tmp/theuth-59-smoke6] [--chrome <binary>] [--puppeteer puppeteer-core]
//
// Or from a runtime that already has a Puppeteer `Page`: `const { smoke } = await import(this file); await smoke(page, opts)`.
// Exit 0 = every assertion held; screenshots and a JSON report land in `--out`.

type Rect = { top: number; bottom: number; left: number; right: number }

// The slice of Puppeteer's `Page` this script uses, so it needs no puppeteer types at compile time.
export type PageLike = {
  emulate(device: { viewport: Record<string, unknown>; userAgent: string }): Promise<void>
  setViewport(v: Record<string, unknown>): Promise<void>
  evaluate<T>(fn: string | ((...a: never[]) => T), ...args: unknown[]): Promise<T>
  goto(url: string, o?: { waitUntil?: string }): Promise<unknown>
  waitForSelector(sel: string, o?: { timeout?: number; visible?: boolean; hidden?: boolean }): Promise<unknown>
  waitForFunction(fn: string | ((...a: never[]) => unknown), o?: { timeout?: number }, ...args: unknown[]): Promise<unknown>
  tap(sel: string): Promise<void>
  click(sel: string): Promise<void>
  hover(sel: string): Promise<void>
  bringToFront(): Promise<void>
  keyboard: { type(text: string, o?: { delay?: number }): Promise<void>; press(key: string): Promise<void>; down(key: string): Promise<void>; up(key: string): Promise<void> }
  screenshot(o: { path: string }): Promise<unknown>
  browser(): { newPage(): Promise<PageLike> }
  reload(o?: { waitUntil?: string }): Promise<unknown>
  close(): Promise<void>
}

export type Opts = { ui: string; rpc: string; token: string; out: string; dataDir: string; words?: number }

const IPHONE = {
  viewport: { width: 390, height: 844, deviceScaleFactor: 3, isMobile: true, hasTouch: true, isLandscape: false },
  userAgent:
    'Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1',
}
const KEYBOARD = 336
/** The last top-level block's content (a trailing ProseMirror widget follows it, so no `:last-child`). */
const LAST_BLOCK = '.bn-editor > .bn-block-group > .bn-block-outer:not(:has(~ .bn-block-outer)) > .bn-block > .bn-block-content'

/** One jsonrpsee call over a fresh socket (the smoke needs two: create the note, read the export). */
async function rpc<T>(url: string, token: string, method: string, params: unknown[]): Promise<T> {
  const ws = new WebSocket(`${url}/?token=${token}`)
  const opened = Promise.withResolvers<void>()
  ws.onopen = () => opened.resolve()
  ws.onerror = (e) => opened.reject(e)
  await opened.promise
  const replied = Promise.withResolvers<T>()
  ws.onmessage = (m) => {
    const f = JSON.parse(String(m.data)) as { id?: number; result?: T; error?: unknown }
    if (f.id !== 1) return
    f.error ? replied.reject(new Error(JSON.stringify(f.error))) : replied.resolve(f.result as T)
  }
  const reply = replied.promise
  ws.send(JSON.stringify({ jsonrpc: '2.0', id: 1, method, params }))
  const r = await reply
  ws.close()
  return r
}

const WORDS =
  'the quick brown fox jumps over the lazy dog while a phone keyboard covers a third of the screen and the toolbar must stay above it'.split(' ')

const report: Record<string, unknown> = {}
const check = (name: string, ok: boolean, detail?: unknown) => {
  report[name] = ok ? (detail ?? 'ok') : { FAILED: detail }
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${name}${detail !== undefined ? ` — ${JSON.stringify(detail)}` : ''}`)
  if (!ok) throw new Error(`check failed: ${name}`)
}
const shot = async (page: PageLike, out: string, name: string) => {
  const path = `${out}/${name}.png`
  await page.screenshot({ path })
  console.log(`     screenshot ${path}`)
}
const rect = (page: PageLike, sel: string) =>
  page.evaluate<Rect | null>(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); if (!e) return null; const r = e.getBoundingClientRect(); return { top: r.top, bottom: r.bottom, left: r.left, right: r.right } })()`)
const cursorBlockRect = (page: PageLike) =>
  page.evaluate<Rect | null>(`(() => {
    const s = getSelection(); if (!s || !s.anchorNode) return null
    const n = s.anchorNode instanceof Element ? s.anchorNode : s.anchorNode.parentElement
    const b = n && n.closest('.bn-block-content'); if (!b) return null
    const r = b.getBoundingClientRect(); return { top: r.top, bottom: r.bottom, left: r.left, right: r.right }
  })()`)
// Inline content without ProseMirror widgets: each side renders the other's cursor label (`showCursorLabels`) in the editor DOM.
const text = (page: PageLike) =>
  page.evaluate<string>(
    `[...document.querySelectorAll('.bn-inline-content')].map((e) => { const c = e.cloneNode(true); c.querySelectorAll('.ProseMirror-widget').forEach((w) => w.remove()); return c.textContent }).join('\\n')`,
  )
// `page.evaluate` may run in an isolated world (it does in the omp browser runtime): a `<script>` element is the
// one way to set a global the app's own code reads.
const mainWorld = (page: PageLike, code: string) =>
  page.evaluate(`(() => { const s = document.createElement('script'); s.textContent = ${JSON.stringify(code)}; document.head.append(s); s.remove() })()`)
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

/** Standalone: both phases on one browser. A runtime that owns emulation per tab runs the two phases from their own tabs. */
export async function smoke(phone: PageLike, o: Opts): Promise<void> {
  const note = await smokePhone(phone, o)
  const desk = await phone.browser().newPage()
  await desk.setViewport({ width: 1280, height: 800, deviceScaleFactor: 1, isMobile: false, hasTouch: false })
  await smokeDesk(desk, o, note, phone)
  await desk.close()
}

/** The iPhone half: creates the note, returns its id. */
export async function smokePhone(phone: PageLike, o: Opts): Promise<string> {
  for (const k of Object.keys(report)) delete report[k]
  const note = await rpc<{ id: string }>(o.rpc, o.token, 'doc_create', ['Mobile smoke', 'Start here.'])
  console.log(`note ${note.id}`)
  await phone.emulate(IPHONE)
  await phone.goto(`${o.ui}/notes/${note.id}`, { waitUntil: 'networkidle0' })
  await phone.waitForSelector('.bn-editor', { timeout: 30_000 })
  await mainWorld(phone, `document.documentElement.dataset.errs = '[]'; const orig = console.error; console.error = (...a) => {
    document.documentElement.dataset.errs = JSON.stringify([...JSON.parse(document.documentElement.dataset.errs), a.map(String).join(' ')]); orig(...a) }`)
  await phone.waitForFunction(`document.querySelector('footer')?.textContent?.includes('live')`, { timeout: 30_000 })
  check('phone: pointer coarse + touch', await phone.evaluate<boolean>(`matchMedia('(pointer: coarse)').matches && navigator.maxTouchPoints > 0`))
  check('phone: no side menu / formatting toolbar / drag handle in the DOM', await phone.evaluate<boolean>(
    `!document.querySelector('.bn-side-menu, .bn-formatting-toolbar, .bn-drag-handle')`,
  ))
  check('phone: toolbar hidden while the editor is not focused', !(await rect(phone, '[data-mobile-toolbar]')))

  // Focus the end of the body paragraph (macOS keybindings: `End` scrolls, ⌘↓ moves the caret to the end), then
  // "raise the keyboard": the inset hook plus the visible band `lib/viewport.ts` would publish on a device.
  await phone.tap(LAST_BLOCK)
  await phone.keyboard.down('Meta')
  await phone.keyboard.press('ArrowDown')
  await phone.keyboard.up('Meta')
  await phone.keyboard.type(' ')
  await phone.evaluate(`(() => { const st = document.createElement('style'); st.textContent = 'html{--visible-band:${844 - KEYBOARD}px !important}'; document.head.append(st) })()`)
  await mainWorld(phone, `window.__theuthKbInset = ${KEYBOARD}; visualViewport.dispatchEvent(new Event('resize'))`)
  await phone.waitForSelector('[data-mobile-toolbar]', { timeout: 5_000 })
  await phone.waitForFunction(`getComputedStyle(document.querySelector('[data-mobile-toolbar]')).bottom === '${KEYBOARD}px'`, { timeout: 5_000 })
  let bar = await rect(phone, '[data-mobile-toolbar]')
  check('phone: toolbar bottom edge sits on the keyboard (844 − 336 = 508)', !!bar && Math.abs(bar.bottom - (844 - KEYBOARD)) <= 1, bar)
  check('phone: toolbar targets ≥ 44 px', await phone.evaluate<boolean>(
    `[...document.querySelectorAll('[data-mobile-toolbar] button')].every(b => { const r = b.getBoundingClientRect(); return r.width >= 44 && r.height >= 44 })`,
  ))
  check('phone: editor text ≥ 16px', await phone.evaluate<number>(`parseFloat(getComputedStyle(document.querySelector('.bn-editor')).fontSize)`) >= 16)
  await shot(phone, o.out, 'phone-toolbar')

  // 200 words with the keyboard up; the cursor's block must stay above the bar.
  const words = o.words ?? 200
  for (let i = 0; i < words; i += 1) await phone.keyboard.type(`${WORDS[i % WORDS.length]} `)
  await sleep(300)
  bar = await rect(phone, '[data-mobile-toolbar]')
  const blk = await cursorBlockRect(phone)
  check('phone: after 200 words the cursor block is above the toolbar', !!bar && !!blk && blk.bottom <= bar.top + 1, { block: blk, bar })
  check('phone: no slash menu after typing "/"', await (async () => {
    await phone.keyboard.type('/')
    await sleep(300)
    const none = await phone.evaluate<boolean>(`!document.querySelector('.bn-suggestion-menu')`)
    await phone.keyboard.press('Backspace')
    return none
  })())
  await shot(phone, o.out, 'phone-typed')

  // Block type sheet → bullet.
  const typeBefore = await phone.evaluate<string>(`getSelection().anchorNode.parentElement.closest('.bn-block-content').dataset.contentType`)
  await phone.tap('[data-mobile-toolbar] button[aria-label="Text"], [data-mobile-toolbar] button[aria-label="Block type"]')
  await phone.waitForSelector('[data-block-type="bullet"]', { visible: true, timeout: 5_000 })
  await shot(phone, o.out, 'phone-block-type-sheet')
  await phone.tap('[data-block-type="bullet"]')
  await phone.waitForSelector('[data-block-type="bullet"]', { hidden: true, timeout: 5_000 })
  await phone.waitForFunction(`document.querySelector('[data-mobile-toolbar] button[aria-label="Bullet list"]')`, { timeout: 5_000 })
  const typeAfter = await phone.evaluate<string>(`getSelection().anchorNode.parentElement.closest('.bn-block-content').dataset.contentType`)
  check('phone: Aa sheet turned the paragraph into a bullet', typeBefore === 'paragraph' && typeAfter === 'bulletListItem', { typeBefore, typeAfter })
  check('phone: toolbar still shown after the sheet closed (focus restored)', !!(await rect(phone, '[data-mobile-toolbar]')))

  // Undo from the toolbar.
  await phone.tap('[data-mobile-toolbar] button[aria-label="Undo"]')
  await phone.waitForFunction(`document.querySelector('[data-mobile-toolbar] button[aria-label="Text"]')`, { timeout: 5_000 })
  check('phone: toolbar Undo restored the paragraph', await phone.evaluate<string>(`getSelection().anchorNode.parentElement.closest('.bn-block-content').dataset.contentType`) === 'paragraph')

  // Bold reflects its active state.
  await phone.tap('[data-mobile-toolbar] button[aria-label="Bold"]')
  await phone.keyboard.type('bold ')
  check('phone: Bold button reflects the active style', await phone.evaluate<boolean>(
    `document.querySelector('[data-mobile-toolbar] button[aria-label="Bold"]').getAttribute('aria-pressed') === 'true' && !!document.querySelector('.bn-editor strong')`,
  ))
  await phone.tap('[data-mobile-toolbar] button[aria-label="Bold"]')

  // Link sheet.
  await phone.tap('[data-mobile-toolbar] button[aria-label="Link"]')
  await phone.waitForSelector('input[aria-label="URL"]', { visible: true, timeout: 5_000 })
  await shot(phone, o.out, 'phone-link-sheet')
  await phone.keyboard.type('https://example.com/mobile')
  await phone.keyboard.press('Enter')
  await phone.waitForSelector('input[aria-label="URL"]', { hidden: true, timeout: 5_000 })
  check('phone: link sheet applied a link', await phone.evaluate<boolean>(`!!document.querySelector('.bn-editor a[href="https://example.com/mobile"]')`))

  // Keyboard-down hides the bar after the grace.
  await phone.tap('[data-mobile-toolbar] button[aria-label="Hide keyboard"]')
  await phone.waitForSelector('[data-mobile-toolbar]', { hidden: true, timeout: 2_000 })
  check('phone: keyboard-down blurred the editor and hid the bar', await phone.evaluate<boolean>(`document.activeElement === document.body`))
  const errs = JSON.parse(await phone.evaluate<string>(`document.documentElement.dataset.errs`)) as string[]
  check('phone: no console.error', errs.length === 0, errs)
  await Bun.write(`${o.out}/report.json`, JSON.stringify(report, null, 1))
  return note.id
}

/** The desktop half on the same note: no toolbar, side menu back, the two editors converge. `phone` is only read. */
export async function smokeDesk(desk: PageLike, o: Opts, note: string, phone: PageLike): Promise<void> {
  await desk.goto(`${o.ui}/notes/${note}`, { waitUntil: 'networkidle0' })
  await desk.waitForSelector('.bn-editor', { timeout: 30_000 })
  const live = `document.querySelector('footer')?.textContent?.includes('live')`
  // A second RPC socket in the same headless Chromium sometimes never opens; one reload has always fixed it.
  await desk.waitForFunction(live, { timeout: 10_000 }).catch(async () => {
    await desk.reload({ waitUntil: 'networkidle0' })
    await desk.waitForSelector('.bn-editor', { timeout: 30_000 })
    await desk.waitForFunction(live, { timeout: 30_000 })
  })
  check('desktop: 1280 px, pointer fine, no mobile toolbar', await desk.evaluate<boolean>(
    `innerWidth === 1280 && matchMedia('(pointer: fine)').matches && !document.querySelector('[data-mobile-toolbar]')`,
  ))
  await desk.bringToFront() // a background page in headless Chromium does not receive key events
  await desk.click(LAST_BLOCK)
  await desk.waitForFunction(`document.activeElement?.classList.contains('bn-editor')`, { timeout: 5_000 })
  await desk.keyboard.type(' from-the-desk')
  check('desktop: typing lands in the editor', await desk.evaluate<boolean>(`document.querySelector('.bn-editor').textContent.includes('from-the-desk')`))
  await desk.hover(LAST_BLOCK)
  check('desktop: side menu present on hover/focus', await desk.evaluate<boolean>(`!!document.querySelector('.bn-side-menu')`))
  await shot(desk, o.out, 'desktop-no-toolbar')
  await sleep(800)
  const [tp, td] = [await text(phone), await text(desk)]
  check('convergence: phone and desktop editors read the same text', tp === td && td.includes('from-the-desk') && td.includes('bold'), { phone: tp.length, desk: td.length })
  await sleep(1500) // export debounce
  const { meta } = await rpc<{ meta: { derived: string } }>(o.rpc, o.token, 'doc_open', [note])
  const md = await Bun.file(`${o.dataDir}/${meta.derived}`).text()
  check('derived markdown contains the typed text', md.includes('from-the-desk') && md.includes('**bold'), md.length)
  // The two phases may run in separate module instances (one per tab runtime): merge with what the phone wrote.
  const prior = (await Bun.file(`${o.out}/report.json`).json().catch(() => ({}))) as Record<string, unknown>
  await Bun.write(`${o.out}/report.json`, JSON.stringify({ ...prior, ...report }, null, 1))
  console.log(`report ${o.out}/report.json`)
}

if (import.meta.main) {
  const args = process.argv.slice(2)
  const flag = (name: string) => {
    const i = args.indexOf(name)
    return i < 0 ? undefined : args[i + 1]
  }
  const o: Opts = {
    ui: flag('--ui') ?? 'http://127.0.0.1:1451',
    rpc: flag('--rpc') ?? 'ws://127.0.0.1:7821',
    token: flag('--token') ?? '',
    out: flag('--out') ?? '/tmp/theuth-59-smoke6',
    dataDir: flag('--data-dir') ?? '/tmp/theuth-59-6',
  }
  // Runtime-selected: puppeteer is not a dependency of the UI; `--puppeteer` names whichever install the caller has.
  const mod = flag('--puppeteer') ?? 'puppeteer-core'
  type Launcher = { launch(o: Record<string, unknown>): Promise<{ newPage(): Promise<PageLike>; close(): Promise<void> }> }
  const { default: puppeteer } = (await import(mod)) as { default: Launcher }
  const browser = await puppeteer.launch({ headless: true, executablePath: flag('--chrome') })
  try {
    await smoke(await browser.newPage(), o)
  } finally {
    await browser.close()
  }
}
