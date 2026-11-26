// Send / Steer pressed while dictating (54 §7, 153 §7.4): the dictation stops, and once its final text lands the box
// is submitted exactly once through ⏎'s path. Drives the real Composer and the real dictation store; the daemon's
// `Dictation*` events are fed by hand.
import '../settings/test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement as h, type ReactNode } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, expect, type Mock, test, vi } from 'vitest'
import { TooltipProvider } from '@/components/ui/tooltip'
import * as dictation from '@/lib/dictation'
import { EMPTY, useDrafts } from '@/lib/drafts'
import type * as Rpc from '@/lib/rpc'
import type { VoiceEvent } from '@/rpc/bindings/event.voice_2eevents.output'
import type { ModelState } from '@/rpc/bindings/method.talk.output'
import { useShortcuts } from '@/lib/shortcuts'
import { useSessions } from '@/store/sessions'
import { Composer } from './Composer'
import { FRESH } from './slash'

const h0 = vi.hoisted(() => {
  /** The calls asserted on; anything else the composer asks resolves `null` (or `[]` for a list). */
  const calls = { dictation_start: vi.fn(), dictation_stop: vi.fn(), send_input: vi.fn(), steer: vi.fn(), cancel: vi.fn() }
  const rest = new Map<string, Mock>()
  const api = new Proxy(calls as Record<string, Mock>, {
    get: (t, k: string) => {
      if (k in t) return t[k]
      if (!rest.has(k)) rest.set(k, vi.fn(async () => (k.startsWith('list_') ? [] : null)))
      return rest.get(k)
    },
  })
  /** The `voice.events@1` handlers `installDictationListeners` registered, by event type. */
  return { calls, api, voice: new Map<string, (e: unknown) => void>() }
})
const api = h0.calls
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => h0.api }))
vi.mock('@tanstack/react-router', async (original) => ({ ...(await original<object>()), useNavigate: () => vi.fn(), useRouterState: () => '/' }))
vi.mock('@/lib/notify', () => ({ notify: Object.assign(vi.fn(), { success: vi.fn(), warning: vi.fn(), error: vi.fn(), message: vi.fn() }) }))
// The voice query is what offers the mic; the rest of `speech` stays real.
vi.mock('@/lib/speech', async (original) => ({ ...(await original<object>()), useVoice: () => true, release: async () => undefined }))
vi.mock('@/store/sessions', async (original) => ({
  ...(await original<object>()),
  on: (_topic: string, type: string, cb: (e: unknown) => void) => {
    h0.voice.set(type, cb)
    return () => h0.voice.delete(type)
  },
}))

const SESSION = 's1'
const SEND = { command: 'send', near_miss: false } as const
let root: Root | null = null
let host: HTMLDivElement
let offListeners: () => void

const emit = async (e: Record<string, unknown>) => {
  await act(async () => h0.voice.get(String(e.type))?.(e as VoiceEvent))
}
const text = (utterance: number, shaped: string, final: boolean, command: typeof SEND | null = null) =>
  emit({ type: 'DictationText', id: 'd1', utterance, text: shaped, shaped, final, command, actions: [] })
const finished = () => emit({ type: 'DictationFinished', id: 'd1', reason: { reason: 'stopped' } })
const box = () => host.querySelector('textarea')!
const sendButton = () => host.querySelector<HTMLButtonElement>('button[aria-label="Send"]')!
const click = async (el: HTMLElement) => {
  await act(async () => el.click())
}

/**
 * The webview's `insertText`/`delete`: edits the focused textarea at its selection and fires `input`, synchronously,
 * so React's `onChange` runs (and flushes) inside the caller — as `replayInto` meets it in WebKit.
 */
function nativeEditing() {
  document.execCommand = ((cmd: string, _ui?: boolean, value?: string) => {
    const ta = document.activeElement
    if (!(ta instanceof window.HTMLTextAreaElement)) return false
    if (cmd !== 'insertText' && cmd !== 'delete') return false
    ta.setRangeText(cmd === 'insertText' ? (value ?? '') : '', ta.selectionStart, ta.selectionEnd, 'end')
    ta.dispatchEvent(new window.InputEvent('input', { bubbles: true, inputType: cmd === 'insertText' ? 'insertText' : 'deleteContentBackward' }))
    return true
  }) as typeof document.execCommand
}

async function mount(running = false) {
  useSessions.getState().loadHistory(SESSION, { ...FRESH, running }, 'replace')
  host = document.createElement('div')
  document.body.append(host)
  root = createRoot(host)
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  await act(async () =>
    root!.render(h(QueryClientProvider, { client: qc }, h(TooltipProvider, null, h(Composer, { session: SESSION, pane: null }) as ReactNode))),
  )
}

/** Start dictating into the focused, empty box and hear `partial` (not yet final). */
async function dictate(partial: string) {
  box().focus()
  await act(async () => window.dispatchEvent(new Event('theuth:dictate')))
  await emit({ type: 'DictationStarted', id: 'd1' })
  expect(dictation.current().phase).toBe('dictating')
  await text(0, partial, false)
}

beforeEach(() => {
  for (const f of Object.values(api)) f.mockReset().mockResolvedValue(null)
  api.dictation_start.mockResolvedValue('d1')
  offListeners = dictation.installDictationListeners()
  dictation.setModel({ state: 'ready' } as ModelState)
  useDrafts.setState((s) => ({ by: { ...s.by, [SESSION]: EMPTY } }))
  nativeEditing()
})

afterEach(async () => {
  // The dictation store is the module's: a test that failed mid-dictation must not leave the next one dictating.
  if (dictation.current().active) await finished()
  act(() => root?.unmount())
  root = null
  host.remove()
  offListeners()
  useSessions.setState((s) => {
    const { [SESSION]: _, ...rest } = s.sessions
    return { sessions: rest }
  })
})

test('Send while dictating stops it, then sends the final text once it lands — once, through the box (native replay)', async () => {
  await mount()
  await dictate('hello wor')
  await click(sendButton())
  expect(api.dictation_stop).toHaveBeenCalledWith('d1')
  // A second press while it stops is the same send, not another.
  await click(sendButton())
  expect(api.dictation_stop).toHaveBeenCalledTimes(1)
  expect(api.send_input).not.toHaveBeenCalled()
  await text(0, 'hello world', true)
  await finished()
  expect(api.send_input).toHaveBeenCalledTimes(1)
  expect(api.send_input.mock.calls[0]!.slice(0, 2)).toEqual([SESSION, 'Hello world'])
})

test('the same when the webview takes no editing command (the box is written instead of replayed)', async () => {
  document.execCommand = (() => false) as typeof document.execCommand
  await mount()
  await dictate('hello wor')
  await click(sendButton())
  await text(0, 'hello world', true)
  await finished()
  expect(api.send_input).toHaveBeenCalledTimes(1)
  expect(api.send_input.mock.calls[0]!.slice(0, 2)).toEqual([SESSION, 'Hello world'])
})

test('Send while dictating, and the final comes back empty: nothing is sent', async () => {
  await mount()
  await dictate('um')
  await click(sendButton())
  await text(0, '', true)
  await finished()
  expect(api.send_input).not.toHaveBeenCalled()
  expect(api.steer).not.toHaveBeenCalled()
})

test('⌘↩ (interrupt) while dictating into a running turn: stops, then interjects the final text — cancel, then steer', async () => {
  await mount(true)
  await dictate('stop that')
  // The interject chord as this platform binds it (`Mod+Enter`).
  const mods = useShortcuts.getState().chords.interject.split('+')
  const init = { key: 'Enter', code: 'Enter', metaKey: mods.includes('Super'), ctrlKey: mods.includes('Control'), altKey: mods.includes('Alt'), shiftKey: mods.includes('Shift'), bubbles: true }
  await act(async () => box().dispatchEvent(new window.KeyboardEvent('keydown', init)))
  expect(api.dictation_stop).toHaveBeenCalledWith('d1')
  await text(0, 'stop that now', true)
  await finished()
  expect(api.cancel).toHaveBeenCalledWith(SESSION, null, 'input')
  expect(api.steer).toHaveBeenCalledTimes(1)
  expect(api.steer.mock.calls[0]!.slice(0, 2)).toEqual([SESSION, 'Stop that now'])
  expect(api.cancel.mock.invocationCallOrder[0]).toBeLessThan(api.steer.mock.invocationCallOrder[0]!)
})

test('spoken "send it": the landed text is sent once without a press', async () => {
  await mount()
  await dictate('ship it')
  await text(0, 'ship it', true, SEND)
  await act(async () => void dictation.stop(h0.api as never))
  await finished()
  expect(api.send_input).toHaveBeenCalledTimes(1)
  expect(api.send_input.mock.calls[0]!.slice(0, 2)).toEqual([SESSION, 'Ship it'])
})
