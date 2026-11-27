import '../settings/test-dom'
import { act, createElement as h } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { TooltipProvider } from '@/components/ui/tooltip'
import { SendButton } from './Composer'
import { HeldStrip } from './HeldStrip'
import { type Uploaded, createUploads, uploadWait } from './uploads'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

/** An upload the test settles by hand, per file. */
function deferred() {
  const runs: { file: Blob; resolve(u: Uploaded): void; reject(e: Error): void }[] = []
  const upload = (file: Blob) => new Promise<Uploaded>((resolve, reject) => runs.push({ file, resolve, reject }))
  return { runs, upload }
}
const png = (name: string) => new File([new Uint8Array([1, 2, 3])], name, { type: 'image/png' })
const flush = () => act(async () => {})

let host: HTMLDivElement
let root: ReturnType<typeof createRoot>
beforeEach(() => {
  URL.createObjectURL = vi.fn(() => 'blob:preview')
  URL.revokeObjectURL = vi.fn()
  host = document.createElement('div')
  document.body.append(host)
  root = createRoot(host)
})
afterEach(() => {
  act(() => root.unmount())
  host.remove()
})

/** The strip for `ctl`'s list as it stands, re-rendered on every change. */
function mount(ctl: ReturnType<typeof createUploads>) {
  const draw = () => root.render(h(HeldStrip, { pastes: new Map(), onChange() {}, onRemove() {}, uploads: ctl.get(), onCancelUpload: ctl.cancel, onRetryUpload: ctl.retry }))
  ctl.subscribe(() => act(draw))
  act(draw)
}
const tiles = () => [...host.querySelectorAll<HTMLElement>('[data-upload]')]

test('a tile per file appears synchronously on add, before any upload settles; send waits meanwhile', () => {
  const { runs, upload } = deferred()
  const done = vi.fn()
  const ctl = createUploads(upload, done)
  mount(ctl)
  ctl.start([png('a.png'), png('b.png')])
  expect(tiles().map((t) => t.dataset.upload)).toEqual(['pending', 'pending'])
  expect(tiles()[0]?.textContent).toContain('a.png')
  expect(host.querySelectorAll('[data-upload-spinner]')).toHaveLength(2)
  expect(runs).toHaveLength(2)
  expect(done).not.toHaveBeenCalled()
  expect(uploadWait(ctl.get())).toBe('Waiting for 2 attachments to upload')
})

test('a landed upload leaves the strip and reaches done; nothing spins once all have landed', async () => {
  const { runs, upload } = deferred()
  const done = vi.fn()
  const ctl = createUploads(upload, done)
  mount(ctl)
  ctl.start([png('a.png'), png('b.png')])
  runs[1]?.resolve({ path: '/att/b.png', dims: [4, 3] })
  await flush()
  expect(done).toHaveBeenCalledWith({ path: '/att/b.png', dims: [4, 3] })
  expect(tiles().map((t) => t.textContent)).toEqual([expect.stringContaining('a.png')])
  runs[0]?.resolve({ path: '/att/a.png' })
  await flush()
  expect(tiles()).toHaveLength(0)
  expect(host.querySelector('[data-upload-spinner], .animate-spin')).toBeNull()
  expect(uploadWait(ctl.get())).toBeNull()
  expect(URL.revokeObjectURL).toHaveBeenCalledTimes(2)
})

test('a failed upload turns into an error tile with retry; retry uploads again and lands', async () => {
  const { runs, upload } = deferred()
  const done = vi.fn()
  const ctl = createUploads(upload, done)
  mount(ctl)
  ctl.start([png('a.png')])
  runs[0]?.reject(new Error('too large'))
  await flush()
  expect(tiles()[0]?.dataset.upload).toBe('error')
  expect(tiles()[0]?.title).toBe('too large')
  expect(host.querySelector('[data-upload-spinner]')).toBeNull()
  expect(uploadWait(ctl.get())).toBeNull()
  act(() => host.querySelector<HTMLButtonElement>('[aria-label="Retry a.png"]')?.click())
  expect(tiles()[0]?.dataset.upload).toBe('pending')
  expect(runs).toHaveLength(2)
  runs[1]?.resolve({ path: '/att/a.png' })
  await flush()
  expect(done).toHaveBeenCalledOnce()
  expect(tiles()).toHaveLength(0)
})

test('✕ cancels: the tile goes at once and the late result is ignored', async () => {
  const { runs, upload } = deferred()
  const done = vi.fn()
  const ctl = createUploads(upload, done)
  mount(ctl)
  ctl.start([png('a.png')])
  act(() => host.querySelector<HTMLButtonElement>('[aria-label="Remove upload of a.png"]')?.click())
  expect(tiles()).toHaveLength(0)
  expect(uploadWait(ctl.get())).toBeNull()
  runs[0]?.resolve({ path: '/att/a.png' })
  await flush()
  expect(done).not.toHaveBeenCalled()
})

test('Send is disabled while an attachment uploads and says why; enabled again after', () => {
  const onSend = vi.fn()
  const draw = (waiting: string | null) => act(() => root.render(h(TooltipProvider, null, h(SendButton, { disabled: false, waiting, onSend }))))
  draw('Waiting for the attachment to upload')
  const send = host.querySelector<HTMLButtonElement>('[aria-label="Send"]')!
  expect(send.disabled).toBe(true)
  expect(host.querySelector('[data-send-waiting]')?.getAttribute('aria-label')).toBe('Waiting for the attachment to upload')
  act(() => send.click())
  expect(onSend).not.toHaveBeenCalled()
  draw(null)
  expect(host.querySelector<HTMLButtonElement>('[aria-label="Send"]')!.disabled).toBe(false)
  expect(host.querySelector('[data-send-waiting]')).toBeNull()
})

test('the composer guards submit on uploads and feeds the strip', async () => {
  const src = (await import('node:fs')).readFileSync(new URL('./Composer.tsx', import.meta.url), 'utf8')
  expect(src).toMatch(/\|\| busy \|\| starting \|\| uploading\) return/)
  expect(src).toContain('uploads={uploads}')
  expect(src).toMatch(/onChange=\{\(e\) => \{\n\s+addBlobs\(/)
})

test('zip, md, html, svg, and extensionless files render as non-image upload tiles and held name chips', async () => {
  const { runs, upload } = deferred()
  const files = [
    new File(['pk'], 'bundle.zip', { type: 'application/zip' }),
    new File(['# hi'], 'notes.md', { type: 'text/markdown' }),
    new File(['<p>hi</p>'], 'page.html', { type: 'text/html' }),
    new File(['<svg/>'], 'icon.svg', { type: 'image/svg+xml' }),
    new File(['all:'], 'Makefile', { type: '' }),
  ]
  const landed = new Map<number, { kind: 'file'; path: string; w: number; h: number }>()
  const draw = () =>
    root.render(h(HeldStrip, { pastes: new Map(landed), onChange() {}, onRemove() {}, uploads: ctl.get(), onCancelUpload: ctl.cancel, onRetryUpload: ctl.retry }))
  const ctl = createUploads(upload, (u) => {
    landed.set(landed.size + 1, { kind: 'file', path: u.path, w: 0, h: 0 })
    draw()
  })
  ctl.subscribe(() => act(draw))
  act(draw)

  ctl.start(files)
  expect(tiles().map((t) => t.textContent)).toEqual(['bundle.zip', 'notes.md', 'page.html', 'icon.svg', 'Makefile'])
  expect(host.querySelectorAll('img')).toHaveLength(0)
  expect(URL.createObjectURL).not.toHaveBeenCalled()

  files.forEach((f, i) => runs[i]?.resolve({ path: `/att/${f.name}` }))
  await flush()

  expect(tiles()).toHaveLength(0)
  expect(host.querySelectorAll('img')).toHaveLength(0)
  expect(host.querySelectorAll('[aria-label^="Open image:"]')).toHaveLength(0)
  for (const f of files) expect(host.textContent).toContain(f.name)
})
