// 53 decisions 9–10 / 03 §0.3: the desktop's own meeting settings — the eye (hidden from capture),
// the two hotkeys, the island's position per screen configuration, the last session model picked
// on the start form, the transcript/chat split of `/meetings/$id` and (156 D3) its brief column's
// width and collapsed state. `localStorage` + zustand like `nudges.ts`; no daemon setting. Each webview
// persists what it reads, and the `capture-hidden` Tauri event keeps the eye in step across windows,
// so nothing depends on the two webviews sharing storage.

import { create } from 'zustand'

const KEY = 'meeting-settings'

export const DEFAULT_PTT = 'Alt+`'
export const DEFAULT_EYE = 'Alt+Shift+E'
/** The transcript's share of the meeting page's left column; the chat below gets the rest. */
export const DEFAULT_SPLIT = 0.6
/** 156 D3: the brief column's width on `/meetings/$id` (desktop), and the range its resize handle allows. */
export const DEFAULT_BRIEF_WIDTH = 272
export const MIN_BRIEF_WIDTH = 200
export const MAX_BRIEF_WIDTH = 480
/** A dragged, nudged or saved width kept inside the handle's range; a non-number reads as the default. */
export const clampBriefWidth = (w: unknown): number =>
  typeof w === 'number' && Number.isFinite(w) ? Math.min(MAX_BRIEF_WIDTH, Math.max(MIN_BRIEF_WIDTH, Math.round(w))) : DEFAULT_BRIEF_WIDTH

type Saved = {
  hidden: boolean
  ptt: string
  eye: string
  /** `island_place` key → logical top-left. */
  positions: Record<string, [number, number]>
  /** `island_place` key → logical size the user dragged; absent = auto-fit. */
  sizes: Record<string, [number, number]>
  /** `StartMeeting.session_model`; `null` = the engine default. */
  model: string | null
  /** Transcript share of the left column on `/meetings/$id`, 0.15–0.85 (the divider's range). */
  split: number
  /** 156 D3: brief column width in px (`MIN_BRIEF_WIDTH`–`MAX_BRIEF_WIDTH`) and whether it is shown. */
  briefWidth: number
  briefsOpen: boolean
}

type MeetingSettings = Saved & {
  setHidden(hidden: boolean): void
  setHotkey(kind: 'ptt' | 'eye', chord: string): void
  setPosition(key: string, pos: [number, number]): void
  /** Forget the saved spot for `key`: the next `island_place` uses the notch default. */
  clearPosition(key: string): void
  setSize(key: string, size: [number, number]): void
  setModel(model: string | null): void
  setSplit(split: number): void
  setBriefWidth(width: number): void
  setBriefsOpen(open: boolean): void
}

/** `split` outside the divider's range (a hand-edited or older value) falls back to the default; `briefWidth` is clamped the same way. */
function load(): Saved {
  const base: Saved = { hidden: true, ptt: DEFAULT_PTT, eye: DEFAULT_EYE, positions: {}, sizes: {}, model: null, split: DEFAULT_SPLIT, briefWidth: DEFAULT_BRIEF_WIDTH, briefsOpen: true }
  try {
    const saved = { ...base, ...(JSON.parse(localStorage.getItem(KEY) ?? '{}') as Partial<Saved>) }
    if (typeof saved.split !== 'number' || !(saved.split >= 0.15 && saved.split <= 0.85)) saved.split = DEFAULT_SPLIT
    saved.briefWidth = clampBriefWidth(saved.briefWidth)
    saved.briefsOpen = saved.briefsOpen !== false
    return saved
  } catch {
    return base
  }
}

export const useMeetingSettings = create<MeetingSettings>()((set, get) => {
  const save = (patch: Partial<Saved>) => {
    const { hidden, ptt, eye, positions, sizes, model, split, briefWidth, briefsOpen } = { ...get(), ...patch }
    localStorage.setItem(KEY, JSON.stringify({ hidden, ptt, eye, positions, sizes, model, split, briefWidth, briefsOpen }))
    set(patch)
  }
  return {
    ...load(),
    setHidden: (hidden) => save({ hidden }),
    setHotkey: (kind, chord) => save({ [kind]: chord }),
    setPosition: (key, pos) => save({ positions: { ...get().positions, [key]: pos } }),
    clearPosition: (key) => {
      const { [key]: _gone, ...positions } = get().positions
      save({ positions })
    },
    setSize: (key, size) => save({ sizes: { ...get().sizes, [key]: size } }),
    setModel: (model) => save({ model }),
    setSplit: (split) => save({ split }),
    setBriefWidth: (width) => save({ briefWidth: clampBriefWidth(width) }),
    setBriefsOpen: (briefsOpen) => save({ briefsOpen }),
  }
})
