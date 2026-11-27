// 224: one PNG per screen of `src/dev/cloud-fixture.html` into docs/design/224-assets/<state>.png — phone states at
// 390×844 (mobile emulation), desktop at 1100×720, both @2x. Drives headless Chrome over the DevTools protocol with
// Bun's own WebSocket (no puppeteer dependency: headless windows cannot go narrower than ~500 px, so the viewport is
// emulated). Targets a running `bunx vite --port 1460 --strictPort`, or starts one for the run.
//
//   bun scripts/cloud-fixture-shots.ts [--ui http://127.0.0.1:1460] [--chrome <binary>] [--theme dark] [--only <state,…>]

import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const flag = (name: string) => {
  const i = process.argv.indexOf(name)
  return i > 0 ? process.argv[i + 1] : undefined
}

const UI = flag('--ui') ?? 'http://127.0.0.1:1460'
const CHROME = flag('--chrome') ?? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'
const DARK = flag('--theme') === 'dark'
const ONLY = flag('--only')?.split(',')
const OUT = resolve(import.meta.dir, '../../../../docs/design/224-assets')

const STATES = [
  'phone-choose',
  'phone-provider',
  'phone-configure',
  'phone-configure-desktop-off',
  'phone-configure-2gb-desktop',
  'phone-configure-price-unavailable',
  'phone-configure-loading',
  'phone-progress',
  'phone-done',
  'phone-engines-entry',
  'desktop-provider',
  'desktop-configure',
  'desktop-k8s',
  'desktop-progress',
  'settings-machines',
  'destroy-confirm',
  'resize',
  'logs',
  'desktop-view',
]

async function up(url: string): Promise<boolean> {
  try {
    return (await fetch(url, { signal: AbortSignal.timeout(1500) })).ok
  } catch {
    return false
  }
}

async function ensureVite(): Promise<(() => void) | null> {
  if (await up(`${UI}/src/dev/cloud-fixture.html`)) return null
  const port = new URL(UI).port || '1460'
  const vite = Bun.spawn(['bunx', 'vite', '--port', port, '--strictPort', '--host', '127.0.0.1'], { cwd: resolve(import.meta.dir, '..'), stdout: 'ignore', stderr: 'inherit' })
  for (let i = 0; i < 60; i++) {
    if (await up(`${UI}/src/dev/cloud-fixture.html`)) return () => vite.kill()
    await Bun.sleep(500)
  }
  vite.kill()
  throw new Error(`vite did not come up on ${UI}`)
}

/** A minimal CDP session on one page target. */
async function cdp(wsUrl: string) {
  const ws = new WebSocket(wsUrl)
  const opened = Promise.withResolvers<void>()
  ws.onopen = () => opened.resolve()
  ws.onerror = (e) => opened.reject(e)
  await opened.promise
  let id = 0
  const waiting = new Map<number, { ok: (v: unknown) => void; fail: (e: Error) => void }>()
  const events: ((m: { method: string; params: unknown }) => void)[] = []
  ws.onmessage = (m) => {
    const f = JSON.parse(String(m.data)) as { id?: number; result?: unknown; error?: { message: string }; method?: string; params?: unknown }
    if (f.id !== undefined) {
      const w = waiting.get(f.id)
      waiting.delete(f.id)
      f.error ? w?.fail(new Error(f.error.message)) : w?.ok(f.result)
    } else if (f.method) for (const cb of events) cb({ method: f.method, params: f.params })
  }
  return {
    send: <T = unknown>(method: string, params: Record<string, unknown> = {}): Promise<T> => {
      const n = ++id
      const reply = Promise.withResolvers<T>()
      waiting.set(n, { ok: (v) => reply.resolve(v as T), fail: reply.reject })
      ws.send(JSON.stringify({ id: n, method, params }))
      return reply.promise
    },
    on: (cb: (m: { method: string; params: unknown }) => void) => events.push(cb),
    close: () => ws.close(),
  }
}

async function launch(): Promise<{ port: number; stop: () => Promise<void> }> {
  const profile = await mkdtemp(join(tmpdir(), 'cloud-shot-'))
  const proc = Bun.spawn([CHROME, '--headless=new', '--disable-gpu', '--hide-scrollbars', '--no-first-run', '--no-default-browser-check', `--user-data-dir=${profile}`, '--remote-debugging-port=0', 'about:blank'], { stdout: 'ignore', stderr: 'ignore' })
  for (let i = 0; i < 100; i++) {
    const port = await readFile(join(profile, 'DevToolsActivePort'), 'utf8').then((s) => Number(s.split('\n')[0]), () => 0)
    if (port) return { port, stop: async () => (proc.kill(), await proc.exited, await rm(profile, { recursive: true, force: true })) }
    await Bun.sleep(100)
  }
  proc.kill()
  throw new Error('chrome did not open its DevTools port')
}

async function shot(port: number, state: string): Promise<string> {
  const phone = state.startsWith('phone-')
  const [width, height] = phone ? [390, 844] : [1100, 720]
  const target = (await (await fetch(`http://127.0.0.1:${port}/json/new?about:blank`, { method: 'PUT' })).json()) as { id: string; webSocketDebuggerUrl: string }
  const page = await cdp(target.webSocketDebuggerUrl)
  const errors: string[] = []
  page.on(({ method, params }) => {
    if (method === 'Runtime.exceptionThrown') errors.push(JSON.stringify(params).slice(0, 400))
  })
  await page.send('Runtime.enable')
  await page.send('Page.enable')
  await page.send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 2, mobile: phone })
  if (phone) await page.send('Emulation.setTouchEmulationEnabled', { enabled: true, maxTouchPoints: 5 })
  await page.send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: DARK ? 'dark' : 'light' }, { name: 'prefers-reduced-motion', value: 'reduce' }] })
  await page.send('Page.navigate', { url: `${UI}/src/dev/cloud-fixture.html?state=${state}${DARK ? '&theme=dark' : ''}` })
  // Rendered when the fixture's root has content and the network is quiet for a moment.
  for (let i = 0; i < 100; i++) {
    const r = await page.send<{ result: { value: boolean } }>('Runtime.evaluate', { expression: "document.querySelector('#root')?.childElementCount > 0 && document.fonts.status === 'loaded'", returnByValue: true })
    if (r.result.value) break
    await Bun.sleep(100)
  }
  await Bun.sleep(900)
  const { data } = await page.send<{ data: string }>('Page.captureScreenshot', { format: 'png' })
  const file = join(OUT, `${state}${DARK ? '-dark' : ''}.png`)
  await writeFile(file, Buffer.from(data, 'base64'))
  page.close()
  await fetch(`http://127.0.0.1:${port}/json/close/${target.id}`)
  if (errors.length) console.error(`${state}: ${errors.join('\n')}`)
  return file
}

await mkdir(OUT, { recursive: true })
const stopVite = await ensureVite()
const chrome = await launch()
try {
  const states = ONLY ?? STATES
  // One at a time: a background tab does not paint, so its screenshot never resolves.
  for (const s of states) console.log(await shot(chrome.port, s))
} finally {
  await chrome.stop()
  stopVite?.()
}
