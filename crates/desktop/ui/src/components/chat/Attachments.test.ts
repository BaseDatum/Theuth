import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { Dialog } from '@/components/ui/dialog'
import { TooltipProvider } from '@/components/ui/tooltip'
import type * as Rpc from '@/lib/rpc'
import type { ContextAttachment } from '@/rpc/bindings/ContextAttachment'
import type * as Conn from '@/store/connection'
import { type Item, Lightbox } from './Attachments'

vi.mock('@tanstack/react-router', () => ({ useNavigate: () => vi.fn() }))
vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({ get_attachment: () => Promise.withResolvers<string>().promise }),
}))
vi.mock('@/store/connection', async (original) => ({
  ...(await original<typeof Conn>()),
  useConnection: (pick: (s: unknown) => unknown) => pick({ info: { data_dir: '/dd' }, api: null }),
}))

const occurrence = (removed: boolean): ContextAttachment => ({
  target: { kind: 'binary', owner: { kind: 'message', seq: 7 }, part: 0, image: 1 },
  name: 'shot.png',
  mime: 'image/png',
  path: '/dd/shot.png',
  removed,
})
const item = (removed: boolean): Item => ({ key: 'o0', name: 'shot.png', path: '/dd/shot.png', image: true, pdf: false, occurrence: occurrence(removed) })
const render = (child: ReactNode) => renderToStaticMarkup(createElement(TooltipProvider, null, createElement(Dialog, { open: true }, child)))

test("Sky's iPad report: a visible Close X, and the removal control is a labelled Trash2 button, never an X", () => {
  const html = render(createElement(Lightbox, { item: item(false), session: 's1', active: true }))
  const close = html.match(/<button[^>]*aria-label="Close"[^>]*>.*?<\/button>/)?.[0]
  expect(close).toContain('class="lucide lucide-x')
  const remove = html.match(/<button[^>]*aria-label="Remove from context"[^>]*>.*?<\/button>/)?.[0]
  expect(remove).toContain('>Remove from context</button>')
  expect(remove).toContain('lucide-trash-2')
  expect(remove).not.toContain('lucide-x')
  expect(html.match(/lucide-x\b/g)).toHaveLength(1)
})

test('a removed occurrence keeps the archive-only note and has no removal control', () => {
  const html = render(createElement(Lightbox, { item: item(true), session: 's1', active: true }))
  expect(html).toContain('Removed from context · archive only')
  expect(html).toContain('Archived original, removed from context: shot.png')
  expect(html).not.toContain('aria-label="Remove')
  expect(html).toContain('aria-label="Close"')
})
