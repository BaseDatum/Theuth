// 53 decisions 9–10 / 03 §0.3: the desktop's own meeting settings — the eye (hidden from capture),
// the two hotkeys, the island's position per screen configuration, the last session model picked
// on the start form, the transcript/chat split of `/meetings/$id`, (156 D3) its brief column's
// width and collapsed state, and the centre/note column split. `localStorage` + zustand like `nudges.ts`;
// no daemon setting. Each webview persists what it reads, and the `capture-hidden` Tauri event keeps the
// eye in step across windows, so nothing depends on the two webviews sharing storage.

import { create } from 'zustand'
import { clampRatio } from '@/lib/resize-drag'

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
/** The centre column's (transcript over chat) share of the centre + note pair on `/meetings/$id`; today's `flex 5 : 4`. */
export const DEFAULT_NOTE_SPLIT = 5 / 9
/** A saved ratio inside the handle's range (0.15–0.85); anything else reads as the default. */
const validRatio = (r: unknown, fallback: number): number => (typeof r === 'number' && r >= 0.15 && r <= 0.85 ? r : fallback)
/**
 * The note handle's drag: the line stays under the pointer. `start` is what the press recorded — the ratio, the
 * pointer x and the centre + note pair's width in px; `x` the pointer now.
 */
export const noteSplitAt = (start: { ratio: number; x: number; pairWidth: number }, x: number): number =>
  clampRatio(start.ratio + (x - start.x) / start.pairWidth)

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
  /** 194: `StartMeeting.session_effort`, the effort of `model`; `null` = Default. */
  effort: string | null
  /** Transcript share of the left column on `/meetings/$id`, 0.15–0.85 (the divider's range). */
  split: number
  /** 156 D3: brief column width in px (`MIN_BRIEF_WIDTH`–`MAX_BRIEF_WIDTH`) and whether it is shown. */
  briefWidth: number
  briefsOpen: boolean
  /** Centre column share of the centre + note pair on `/meetings/$id`, 0.15–0.85 (the handle's range). */
  noteSplit: number
}

type MeetingSettings = Saved & {
  setHidden(hidden: boolean): void
  setHotkey(kind: 'ptt' | 'eye', chord: string): void
  setPosition(key: string, pos: [number, number]): void
  /** Forget the saved spot for `key`: the next `island_place` uses the notch default. */
  clearPosition(key: string): void
  setSize(key: string, size: [number, number]): void
  /** The model, with the effort it keeps (194 D4c: `null` unless the new model takes the old one). */
  setModel(model: string | null, effort?: string | null): void
  setEffort(effort: string | null): void
  setSplit(split: number): void
  setBriefWidth(width: number): void
  setBriefsOpen(open: boolean): void
  setNoteSplit(split: number): void
}

/** `split`/`noteSplit` outside the handle range (a hand-edited or older value) fall back to the default; `briefWidth` is clamped the same way. */
function load(): Saved {
  const base: Saved = { hidden: true, ptt: DEFAULT_PTT, eye: DEFAULT_EYE, positions: {}, sizes: {}, model: null, effort: null, split: DEFAULT_SPLIT, briefWidth: DEFAULT_BRIEF_WIDTH, briefsOpen: true, noteSplit: DEFAULT_NOTE_SPLIT }
  try {
    const saved = { ...base, ...(JSON.parse(localStorage.getItem(KEY) ?? '{}') as Partial<Saved>) }
    saved.split = validRatio(saved.split, DEFAULT_SPLIT)
    saved.noteSplit = validRatio(saved.noteSplit, DEFAULT_NOTE_SPLIT)
    saved.briefWidth = clampBriefWidth(saved.briefWidth)
    saved.briefsOpen = saved.briefsOpen !== false
    return saved
  } catch {
    return base
  }
}

export const useMeetingSettings = create<MeetingSettings>()((set, get) => {
  const save = (patch: Partial<Saved>) => {
    const { hidden, ptt, eye, positions, sizes, model, effort, split, briefWidth, briefsOpen, noteSplit } = { ...get(), ...patch }
    localStorage.setItem(KEY, JSON.stringify({ hidden, ptt, eye, positions, sizes, model, effort, split, briefWidth, briefsOpen, noteSplit }))
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
    setModel: (model, effort = null) => save({ model, effort: model ? effort : null }),
    setEffort: (effort) => save({ effort }),
    setSplit: (split) => save({ split }),
    setBriefWidth: (width) => save({ briefWidth: clampBriefWidth(width) }),
    setBriefsOpen: (briefsOpen) => save({ briefsOpen }),
    setNoteSplit: (split) => save({ noteSplit: clampRatio(split) }),
  }
})
