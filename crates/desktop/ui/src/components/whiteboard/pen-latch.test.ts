// The ink tool latches (owner report, iPad + Pencil): once a pen is picked it stays until another tool, select or Esc —
// never cleared by lifting the Pencil, a stroke ending, the cluster hiding or a native picker echo.
// A mini whiteboard shell: the real store, cluster timer, Cluster (Radix popover) and native-ink connection, with a fake
// plugin whose capture recogniser covers the whole web view as on the iPad (route A): while it is enabled a Pencil touch
// anywhere — the cluster's buttons too — is a native stroke, and the page's tap (its click) is cancelled.
import '../settings/test-dom'
import { act, createElement as h } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useStore } from 'zustand'
import type { InkEvent, NativeInk, NativeTool, attachNativeInk } from '@/lib/ink-native'
import { Cluster } from './Cluster'
import { routePointer } from './canvas/input'
import { createWhiteboardStore, type WbStore } from './canvas/store'
import { type ClusterTimer, clusterTimer } from './cluster-timer'
import { type NativeConnection, connectNativeInk } from './native-ink'

type Fake = { enabled: boolean; tools: NativeTool[]; emit(e: InkEvent): void }

function fakeAttach(fake: Fake): typeof attachNativeInk {
  return async (on) => {
    fake.emit = (e) => on.event(e)
    const ink: NativeInk = {
      result: { native: true, route: 'A' },
      setEnabled: (on) => {
        fake.enabled = on
      },
      setViewport() {},
      setTool: (t) => {
        fake.tools.push(t)
      },
      setPickerVisible() {},
      ack() {},
      recognizeText: async () => null,
      detach: async () => {},
    }
    return ink
  }
}

let root: Root
let host: HTMLDivElement
let store: WbStore
let fake: Fake
let conn: NativeConnection | null
let timer: ClusterTimer

function Shell({ store: s }: { store: WbStore }) {
  const open = useStore(s, (v) => v.clusterOpen)
  if (!open) return null
  return h(Cluster, {
    store: s, onActivity: () => timer.activity(), onAdd() {}, onFit() {}, onOutline() {}, onUndo() {}, onRedo() {},
    canUndo: false, canRedo: false, more: null, onHold: timer.hold,
    addable: { sticky: true, note: true, artifact: true, excerpt: true, diff: true, terminal: true, web: true, image: true, clipboard: true },
  })
}

const button = (label: string) => document.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)
/** jsdom has no PointerEvent: a MouseEvent of the pointer type, carrying `pointerType` and `pointerId`. */
const pointer = (el: Element, type: string, pointerType: string) => {
  const e = new MouseEvent(type, { bubbles: true, cancelable: true, button: 0 })
  Object.defineProperties(e, { pointerType: { value: pointerType }, pointerId: { value: 7 } })
  return el.dispatchEvent(e)
}

/** A Pencil tap on a page element. With native capture on, the plugin takes the touch: a stroke, no click. */
function penTap(el: Element) {
  act(() => {
    if (fake.enabled) {
      fake.emit({ kind: 'state', drawing: true })
      fake.emit({ kind: 'state', drawing: false })
      return
    }
    pointer(el, 'pointerdown', 'pen')
    pointer(el, 'pointerup', 'pen')
    el.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }))
  })
}

/** A Pencil stroke on the canvas: native when capture is on, else the pointer path's pen-down / pen-up. */
function penStroke() {
  act(() => {
    if (fake.enabled) {
      fake.emit({ kind: 'state', drawing: true })
      fake.emit({ kind: 'state', drawing: false })
    } else {
      store.getState().set({ penDown: true })
      store.getState().set({ penDown: false })
    }
  })
}

/** Where the next Pencil stroke on the canvas goes. */
const penRoute = () => {
  const s = store.getState()
  return routePointer('pen', { mode: s.mode, fingerInk: s.fingerInk, nativeInk: s.nativeInk && s.inkEngine === 'auto', readOnly: false, penDown: s.penDown, palmUntil: 0, now: 0 })
}

/** Raise the cluster with a finger tap on the canvas and open the ink tools with a finger. */
async function openInkTools() {
  act(() => timer.tap())
  const trigger = button('Ink tools: Pen (P)') ?? button('Ink tools: Fine liner (P)') ?? button('Ink tools: Marker (P)') ?? button('Ink tools: eraser (E)')
  expect(trigger).not.toBeNull()
  await act(async () => {
    pointer(trigger!, 'pointerdown', 'touch')
    pointer(trigger!, 'pointerup', 'touch')
    trigger!.click()
  })
  expect(button('Pen')).not.toBeNull()
}

beforeEach(async () => {
  vi.useFakeTimers()
  host = document.createElement('div')
  document.body.append(host)
  store = createWhiteboardStore()
  fake = { enabled: false, tools: [], emit() {} }
  timer = clusterTimer({ onChange: (open) => store.getState().set({ clusterOpen: open }) })
  store.subscribe((s, p) => s.penDown && !p.penDown && timer.pen())
  conn = await connectNativeInk({ store, handle: () => null, dark: () => false, showTools: () => timer.show(), attach: fakeAttach(fake) })
  root = createRoot(host)
  act(() => root.render(h(Shell, { store })))
})

afterEach(() => {
  act(() => root.unmount())
  conn?.dispose()
  timer.dispose()
  host.remove()
  document.body.innerHTML = ''
  vi.useRealTimers()
})

describe('pen tool latch (iPad, Pencil)', () => {
  it('a Pencil tap on the Pen item picks it: the native recogniser is off while the cluster is up', async () => {
    expect(fake.enabled).toBe(true)
    await openInkTools()
    expect(fake.enabled).toBe(false)
    expect(store.getState().nativeInk).toBe(false)
    penTap(button('Pen')!)
    expect(store.getState()).toMatchObject({ mode: 'draw', tool: 'pen' })
    expect(button('Pen')?.getAttribute('aria-pressed')).toBe('true')
  })

  it('tap the Pen item with the Pencil → lift → the next stroke draws, and the tool stays', async () => {
    await openInkTools()
    penTap(button('Pen')!)
    // The stroke on the canvas (pointer path while the cluster is up) hides the cluster; native capture comes back.
    expect(penRoute()).toBe('ink')
    penStroke()
    expect(store.getState().clusterOpen).toBe(false)
    expect(fake.enabled).toBe(true)
    expect(penRoute()).toBe('swallow')
    expect(store.getState()).toMatchObject({ mode: 'draw', tool: 'pen' })
  })

  it('stroke → lift → the next stroke draws: pen-up never changes the tool', async () => {
    await openInkTools()
    penTap(button('Marker')!)
    for (let i = 0; i < 3; i++) {
      penStroke()
      expect(store.getState()).toMatchObject({ mode: 'draw', tool: 'marker', penDown: false })
      expect(penRoute()).toBe('swallow')
      expect(fake.tools.at(-1)?.tool).toBe('marker')
    }
  })

  it('picking the armed pen again keeps it (a repeated or re-fired pick never switches it off)', async () => {
    await openInkTools()
    penTap(button('Pen')!)
    penTap(button('Pen')!)
    expect(store.getState()).toMatchObject({ mode: 'draw', tool: 'pen' })
    penTap(button('Eraser')!)
    penTap(button('Eraser')!)
    expect(store.getState().mode).toBe('erase')
  })

  it('the cluster auto-hiding (4 s) and the popover closing do not change the tool', async () => {
    await openInkTools()
    penTap(button('Fine liner')!)
    act(() => {
      document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
    })
    act(() => vi.advanceTimersByTime(5_000))
    expect(store.getState().clusterOpen).toBe(false)
    expect(store.getState()).toMatchObject({ mode: 'draw', tool: 'monoline' })
    expect(fake.enabled).toBe(true)
  })

  it('a native `tool` echo on pen-up does not clear the tool', async () => {
    await openInkTools()
    penTap(button('Pen')!)
    act(() => timer.hide())
    const sent = fake.tools.at(-1)!
    penStroke()
    act(() => fake.emit({ kind: 'tool', tool: sent.tool, colour: sent.colour, width: sent.width }))
    expect(store.getState()).toMatchObject({ mode: 'draw', tool: 'pen', colour: 'ink', width: 2.8 })
    // A real pick in the system picker changes the pen and keeps it armed.
    act(() => fake.emit({ kind: 'tool', tool: 'marker', colour: sent.colour, width: 6 }))
    expect(store.getState()).toMatchObject({ mode: 'draw', tool: 'marker', width: 6 })
  })
})
