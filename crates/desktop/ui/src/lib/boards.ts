// Settings › General › Serif prose on boards (71 §0 Q1): board prose is set in Newsreader (`--font-serif`) unless the
// reader opts out, in which case the prose register falls back to `--font-sans`. Chrome stays mono either way.

import { create } from 'zustand'

const KEY = 'boards.serif'

type BoardsStore = {
  serif: boolean
  setSerif(on: boolean): void
}

export const useBoards = create<BoardsStore>()((set) => ({
  // `globalThis.` so the module loads under vitest's node environment.
  serif: globalThis.localStorage?.getItem(KEY) !== 'false',
  setSerif(on) {
    localStorage.setItem(KEY, String(on))
    set({ serif: on })
  },
}))
