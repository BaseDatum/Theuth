// Export ▸ Copy Markdown: the clipboard helper against stubbed clipboards; the menu as static markup.
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { afterEach, expect, test, vi } from 'vitest'
import { copyMarkdown, ExportMenu } from './ExportMenu'

const { pass } = vi.hoisted(() => ({ pass: ({ children }: { children: ReactNode }) => children }))
vi.mock('@/lib/rpc', () => ({ useApi: () => ({}) }))
vi.mock('@/link/engines', () => ({ useEngines: () => null }))
vi.mock('@tanstack/react-router', () => ({ useNavigate: () => () => {} }))
vi.mock('@/components/ui/tooltip', () => ({ Tooltip: pass, TooltipTrigger: pass, TooltipContent: () => null }))
vi.mock('@/components/ui/dropdown-menu', () => ({
  DropdownMenu: pass,
  DropdownMenuTrigger: () => null,
  DropdownMenuContent: ({ children }: { children: ReactNode }) => createElement('div', { role: 'menu' }, children),
  DropdownMenuItem: ({ children }: { children: ReactNode }) => createElement('div', { role: 'menuitem' }, children),
  DropdownMenuSeparator: () => createElement('hr'),
}))

class Item {
  constructor(readonly items: { 'text/plain': Promise<Blob> }) {}
}
/** A clipboard whose `write` resolves every promised item (or fails with `writeError`); records what landed. */
function stub({ write = true, item = true, writeError }: { write?: boolean; item?: boolean; writeError?: Error } = {}) {
  const landed: [string, string][] = []
  const clipboard = {
    writeText: async (t: string) => void landed.push(['writeText', t]),
    write: write
      ? async ([i]: [Item]) => {
          const blob = await i.items['text/plain']
          if (writeError) throw writeError
          landed.push(['write', await blob.text()])
        }
      : undefined,
  }
  vi.stubGlobal('navigator', { clipboard })
  if (item) vi.stubGlobal('ClipboardItem', Item)
  else vi.stubGlobal('ClipboardItem', undefined)
  return landed
}
afterEach(() => vi.unstubAllGlobals())

test('with ClipboardItem: one clipboard.write of the promised text/plain blob, no writeText', async () => {
  const landed = stub()
  await copyMarkdown(async () => '# Note\n')
  expect(landed).toEqual([['write', '# Note\n']])
})

test('the write starts before the markdown arrives (WebKit: inside the gesture)', () => {
  stub()
  const write = vi.spyOn(navigator.clipboard, 'write' as never)
  void copyMarkdown(() => new Promise<string>(() => {}))
  expect(write).toHaveBeenCalledOnce()
})

test('no ClipboardItem or no clipboard.write: writeText after the fetch', async () => {
  const noItem = stub({ item: false })
  await copyMarkdown(async () => 'a')
  expect(noItem).toEqual([['writeText', 'a']])
  const noWrite = stub({ write: false })
  await copyMarkdown(async () => 'b')
  expect(noWrite).toEqual([['writeText', 'b']])
})

test('write fails other than NotAllowed: writeText; NotAllowed rejects as is', async () => {
  const landed = stub({ writeError: new DOMException('type unsupported', 'DataError') })
  await copyMarkdown(async () => 'c')
  expect(landed).toEqual([['writeText', 'c']])
  const denied = stub({ writeError: new DOMException('no gesture', 'NotAllowedError') })
  await expect(copyMarkdown(async () => 'd')).rejects.toThrow('no gesture')
  expect(denied).toEqual([])
})

test('doc_markdown fails: rejects with its error on both paths, nothing copied', async () => {
  const landed = stub()
  await expect(copyMarkdown(() => Promise.reject(new Error('document not found')))).rejects.toThrow('document not found')
  stub({ item: false })
  await expect(copyMarkdown(() => Promise.reject(new Error('document not found')))).rejects.toThrow('document not found')
  expect(landed).toEqual([])
})

test('the menu: Markdown…, PDF…, a separator, Copy Markdown', () => {
  const html = renderToStaticMarkup(createElement(ExportMenu, { id: 'd1', title: 'Note' }))
  expect(html).toBe('<div role="menu"><div role="menuitem">Markdown…</div><div role="menuitem">PDF…</div><hr/><div role="menuitem">Copy Markdown</div></div>')
})
