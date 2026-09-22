// 53 decisions 9–10 / 03 §0.3: the desktop's own meeting settings — the eye (hidden from capture),
// the two hotkeys, the island's position per screen configuration and the last session model picked
// on the start form. `localStorage` + zustand like `nudges.ts`; no daemon setting. Each webview
// persists what it reads, and the `capture-hidden` Tauri event keeps the eye in step across windows,
// so nothing depends on the two webviews sharing storage.

import { create } from 'zustand'

const KEY = 'meeting-settings'

export const DEFAULT_PTT = 'Alt+`'
export const DEFAULT_EYE = 'Alt+Shift+E'

type Saved = {
  hidden: boolean
  ptt: string
  eye: string
  /** `island_place` key → logical top-left. */
  positions: Record<string, [number, number]>
  /** `island_place` key → logical size the user dragged; absent = auto-fit. */
  sizes: Record<string, [number, number]>
  /** `StartMeeting.session_model`; `null` = the harness default. */
  model: string | null
}

type MeetingSettings = Saved & {
  setHidden(hidden: boolean): void
  setHotkey(kind: 'ptt' | 'eye', chord: string): void
  setPosition(key: string, pos: [number, number]): void
  setSize(key: string, size: [number, number]): void
  setModel(model: string | null): void
}

function load(): Saved {
  const base: Saved = { hidden: true, ptt: DEFAULT_PTT, eye: DEFAULT_EYE, positions: {}, sizes: {}, model: null }
  try {
    return { ...base, ...(JSON.parse(localStorage.getItem(KEY) ?? '{}') as Partial<Saved>) }
  } catch {
    return base
  }
}

export const useMeetingSettings = create<MeetingSettings>()((set, get) => {
  const save = (patch: Partial<Saved>) => {
    const { hidden, ptt, eye, positions, sizes, model } = { ...get(), ...patch }
    localStorage.setItem(KEY, JSON.stringify({ hidden, ptt, eye, positions, sizes, model }))
    set(patch)
  }
  return {
    ...load(),
    setHidden: (hidden) => save({ hidden }),
    setHotkey: (kind, chord) => save({ [kind]: chord }),
    setPosition: (key, pos) => save({ positions: { ...get().positions, [key]: pos } }),
    setSize: (key, size) => save({ sizes: { ...get().sizes, [key]: size } }),
    setModel: (model) => save({ model }),
  }
})
