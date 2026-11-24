// Node environment (no DOM): the wire is driven through `attachSource` with a fake xterm; the key row is checked as
// static markup (171 §7.4, §11.6).
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { beforeEach, expect, test, vi } from 'vitest'
import type { PtyChunk } from "@/rpc/bindings/method.agent.output"
import { RpcError } from '@/rpc/client'
import { attachSource, CELL_KEYS, liveRows, Terminal } from './TerminalViewer'

const api = { cell_input: vi.fn(async () => null), pty_input: vi.fn(async () => null), pty_resize: vi.fn(async () => null) }
const unsub = vi.fn(async () => undefined)
const client = { stream: vi.fn(async <T,>(_m: string, _p: unknown[], _u: string, _on: (c: T) => void) => unsub) }
const touch = vi.fn(() => false)

vi.mock('@xterm/xterm', () => ({ Terminal: class {} }))
vi.mock('@xterm/addon-fit', () => ({ FitAddon: class {} }))
vi.mock('@/lib/rpc', () => ({ useApi: () => api, queryKeys: { terminals: (s: string) => ['terminals', s] } }))
vi.mock('@/store/connection', () => ({ useConnection: (pick: (s: unknown) => unknown) => pick({ client , discovery: { phase: 'legacy' } }) }))
vi.mock('@/store/sessions', () => ({ useSessionField: () => null }))
vi.mock('@/lib/platform', async (original) => ({ ...(await original<object>()), touch: () => touch() }))
vi.mock('@/lib/chord', () => ({ isApple: () => true }))
vi.mock('@/lib/owner', () => ({ useEngineLabel: (owner: string) => owner }))
vi.mock('@/lib/popout', () => ({ canPopout: () => false, openPopout: async () => undefined }))
vi.mock('@/hooks/use-mobile', () => ({ useIsMobile: () => false }))

/** The slice of xterm `attachSource` touches. */
const fakeTerm = () => {
  const t = {
    rows: 30,
    cols: 100,
    data: null as ((d: string) => void) | null,
    keys: null as ((e: KeyboardEvent) => boolean) | null,
    reset: vi.fn(),
    resize: vi.fn(),
    write: vi.fn(),
    onData(cb: (d: string) => void) {
      t.data = cb
      return { dispose: vi.fn() }
    },
    attachCustomKeyEventHandler(cb: (e: KeyboardEvent) => boolean) {
      t.keys = cb
    },
    parser: { registerCsiHandler: vi.fn(), registerOscHandler: vi.fn() },
  }
  return t
}

type Term = Parameters<typeof attachSource>[1]
/** Drains the input lane's microtasks (one macrotask turn, not a duration). */
const flush = () => {
  const { promise, resolve } = Promise.withResolvers<void>()
  setTimeout(resolve)
  return promise
}

const onItem = () => client.stream.mock.calls[0]![3] as (c: PtyChunk) => void

beforeEach(() => {
  vi.clearAllMocks()
  touch.mockReturnValue(false)
})

test('a cell source attaches with cell_attach/cell_detach and types through cell_input as base64 UTF-8', async () => {
  const term = fakeTerm()
  const onExit = vi.fn()
  const wire = attachSource({ kind: 'cell', run: 'run-1' }, term as unknown as Term, api, client, onExit)
  expect(client.stream).toHaveBeenCalledTimes(1)
  expect(client.stream.mock.calls[0]!.slice(0, 3)).toEqual(['cell_attach', ['run-1'], 'cell_detach'])
  expect(term.parser.registerCsiHandler).toHaveBeenCalled()

  term.data!('é')
  // 167 D1 through the same lane: ⌥← on a Mac.
  term.keys!({ type: 'keydown', code: 'ArrowLeft', altKey: true, metaKey: false, ctrlKey: false, shiftKey: false, preventDefault() {} } as unknown as KeyboardEvent)
  await flush()
  expect(api.cell_input.mock.calls).toEqual([
    ['run-1', 'w6k='],
    ['run-1', btoa('\x1bb')],
  ])
  expect(api.pty_input).not.toHaveBeenCalled()

  onItem()({ type: 'reset', data: btoa('hi'), rows: 24, cols: 80 })
  expect(term.reset).toHaveBeenCalledTimes(1)
  expect(term.resize).toHaveBeenCalledWith(80, 24)
  expect(term.write).toHaveBeenCalledTimes(1)
  onItem()({ type: 'data', data: btoa('x') })
  expect(term.write).toHaveBeenCalledTimes(2)

  onItem()({ type: 'exit', exit: { exited: 0 } })
  onItem()({ type: 'exit', exit: { exited: 0 } })
  expect(onExit).toHaveBeenCalledTimes(1)
  expect(wire.ended()).toBe(true)
  term.data!('after')
  wire.send('after')
  await flush()
  expect(api.cell_input).toHaveBeenCalledTimes(2)

  wire.dispose()
  await flush()
  expect(unsub).toHaveBeenCalledTimes(1)
  expect(api.pty_resize).not.toHaveBeenCalled()
})

test('the pty source still goes through subscribe_pty and pty_input', async () => {
  const term = fakeTerm()
  attachSource({ kind: 'pty', session: 's', job: 'j' }, term as unknown as Term, api, client, () => {})
  expect(client.stream.mock.calls[0]!.slice(0, 3)).toEqual(['subscribe_pty', ['s', 'j', 30, 100], 'unsubscribe_pty'])
  term.data!('x')
  await flush()
  expect(api.pty_input).toHaveBeenCalledWith('s', 'j', 'x', null)
  expect(api.cell_input).not.toHaveBeenCalled()
})

test('the cell key row: ↑ ↓ ⏎ Esc Tab ^C raw bytes, arrows per cursor-key mode, 48 px targets, only under touch (171 §11.6)', () => {
  const seq = Object.fromEntries(CELL_KEYS)
  expect(seq['↑']!(false)).toBe('\x1b[A')
  expect(seq['↑']!(true)).toBe('\x1bOA')
  expect(seq['↓']!(false)).toBe('\x1b[B')
  expect([seq['⏎']!(false), seq['Esc']!(false), seq['Tab']!(false), seq['^C']!(false)]).toEqual(['\r', '\x1b', '\t', '\x03'])
  expect(btoa(seq['↑']!(false))).toBe('G1tB')

  const cell = () => renderToStaticMarkup(createElement(Terminal, { source: { kind: 'cell', run: 'run-1' }, label: 'python' }))
  expect(cell()).not.toContain('Terminal keys')
  touch.mockReturnValue(true)
  const html = cell()
  expect(html).toContain('role="group" aria-label="Terminal keys"')
  for (const [text] of CELL_KEYS) expect(html).toContain(`>${text}<`)
  expect(html).toContain('h-12 min-w-12')
  expect(html).not.toContain('h-11')
  expect(html).toContain('>python<')
  // The pty source keeps its own strip: on a desktop (useIsMobile false) nothing, even under touch.
  expect(renderToStaticMarkup(createElement(Terminal, { session: 's', job: 'j', label: 'sh' }))).not.toContain('Terminal keys')
  expect(api.pty_resize).not.toHaveBeenCalled()
})

test('171F B12: a cell terminal opens at 12 rows and grows with the rows in use to the 30-row grid; a pty box keeps its own size', () => {
  expect([liveRows(1, 12, 30), liveRows(12, 12, 30), liveRows(20, 12, 30), liveRows(30, 12, 30), liveRows(31, 12, 30)]).toEqual([12, 12, 20, 30, 30])
  const cell = renderToStaticMarkup(createElement(Terminal, { source: { kind: 'cell', run: 'run-1' }, label: 'bash' }))
  // The rows are the xterm host's height (12 × ~15 px); the box around it has none of its own.
  expect(cell).toContain('height:180px')
  expect(renderToStaticMarkup(createElement(Terminal, { session: 's', job: 'j', label: 'sh' }))).not.toContain('height:')
})

test('the cursor row of a waiting cell stays in view: the box grows by its scrollbar and scrolls from the bottom', () => {
  const cell = renderToStaticMarkup(createElement(Terminal, { source: { kind: 'cell', run: 'run-1' }, label: 'bash' }))
  const box = cell.match(/^<div class="(?<cls>[^"]*)" style="(?<style>[^"]*)">/)?.groups
  expect(box, cell).toBeDefined()
  const { cls = '', style = '' } = box ?? {}
  // No fixed height: a horizontal scrollbar (100 cols in a narrow box) adds to the box instead of covering the last row.
  expect(style).not.toContain('height')
  // Clipped by the host's max-height it scrolls, starting at — and staying on — the bottom row (where a prompt waits).
  expect(cls.split(' ')).toEqual(expect.arrayContaining(['overflow-auto', 'flex', 'flex-col-reverse']))
  expect(cls).not.toContain('overflow-hidden')
})

test('CELLHIST: a cell terminal (inside a note editor) has no live region; a pty terminal keeps its polite status line', () => {
  expect(renderToStaticMarkup(createElement(Terminal, { source: { kind: 'cell', run: 'run-1' }, label: 'bash' }))).not.toContain('aria-live')
  expect(renderToStaticMarkup(createElement(Terminal, { source: { kind: 'pty', session: 's', job: 'j' }, label: 'sh' }))).toContain('aria-live="polite"')
  expect(renderToStaticMarkup(createElement(Terminal, { session: 's', job: 'j', label: 'sh' }))).toContain('aria-live="polite"')
})

test('183 §4.3: an attach refused -32014 RunsElsewhere names the owner, sends nothing and raises no error (pty and cell)', async () => {
  for (const src of [{ kind: 'cell', run: 'r' } as const, { kind: 'pty', session: 's', job: 'j' } as const]) {
    client.stream.mockImplementationOnce(async () => {
      throw new RpcError(-32014, 'runs elsewhere', { RunsElsewhere: { owner: 'box' } })
    })
    const term = fakeTerm()
    const where = vi.fn()
    const wire = attachSource(src, term as unknown as Term, api, client, () => {}, where)
    await flush()
    wire.send('x')
    await flush()
    expect(where).toHaveBeenCalledWith('box')
    expect(wire.ended()).toBe(true)
    wire.dispose() // the failed attach left nothing to detach
    await flush()
  }
  expect(api.cell_input).not.toHaveBeenCalled()
  expect(api.pty_input).not.toHaveBeenCalled()
})
