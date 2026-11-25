import { describe, expect, it, vi } from 'vitest'
import type { PencilAction } from '@/lib/ink-native'
import { createWhiteboardStore } from './canvas/store'
import { pencilGestures } from './pencil-gestures'

const ev = (action: PencilAction, gesture: 'tap' | 'squeeze' = 'tap') => ({ kind: 'pencil' as const, gesture, action })

function rig() {
  const store = createWhiteboardStore()
  const showTools = vi.fn()
  const g = pencilGestures(store, showTools)
  return { store, showTools, g, st: () => store.getState() }
}

describe('Pencil double-tap / squeeze → the web tool', () => {
  it('switchEraser toggles the eraser and back to the latched pen, armed', () => {
    const { store, g, st } = rig()
    st().set({ tool: 'marker', colour: 'red', mode: 'draw' })
    g.on(ev('switchEraser'))
    expect(st()).toMatchObject({ mode: 'erase', tool: 'marker' })
    g.on(ev('switchEraser', 'squeeze'))
    expect(st()).toMatchObject({ mode: 'draw', tool: 'marker', colour: 'red' })
    g.dispose()
    expect(store.getState().mode).toBe('draw')
  })

  it('switchPrevious swaps with the previous tool; twice returns to the start', () => {
    const { g, st } = rig()
    st().set({ tool: 'monoline', mode: 'draw' })
    st().set({ tool: 'marker', mode: 'draw' })
    g.on(ev('switchPrevious'))
    expect(st()).toMatchObject({ tool: 'monoline', mode: 'draw' })
    g.on(ev('switchPrevious'))
    expect(st()).toMatchObject({ tool: 'marker', mode: 'draw' })
    // From the eraser: previous is the pen it left.
    st().set({ mode: 'erase' })
    g.on(ev('switchPrevious'))
    expect(st()).toMatchObject({ tool: 'marker', mode: 'draw' })
    g.on(ev('switchPrevious'))
    expect(st().mode).toBe('erase')
  })

  it('the palette actions raise the cluster and leave the tool', () => {
    const { g, st, showTools } = rig()
    st().set({ tool: 'pen', mode: 'draw' })
    for (const a of ['showColorPalette', 'showInkAttributes', 'showContextualPalette'] as const) g.on(ev(a, 'squeeze'))
    expect(showTools).toHaveBeenCalledTimes(3)
    expect(st()).toMatchObject({ tool: 'pen', mode: 'draw' })
  })

  it('ignore and runSystemShortcut change nothing; the tool stays latched after any gesture', () => {
    const { g, st, showTools } = rig()
    st().set({ tool: 'monoline', colour: 'blue', width: 4, mode: 'draw' })
    const before = { ...st() }
    g.on(ev('ignore'))
    g.on(ev('runSystemShortcut', 'squeeze'))
    expect(showTools).not.toHaveBeenCalled()
    expect(st()).toMatchObject({ tool: before.tool, colour: before.colour, width: before.width, mode: before.mode })
  })
})
