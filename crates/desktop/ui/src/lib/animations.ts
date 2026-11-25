// Settings › General › Appearance › Animations: client-only global toggle for interface motion (Paper & Ink
// scroll-area entrances, consecutive tool-run fold/expand, Folded Horizon session bar warp). Always defers to
// `prefers-reduced-motion: reduce`.

import { create } from 'zustand'

const KEY = 'ui.animations'

type AnimationsStore = {
  enabled: boolean
  setEnabled(on: boolean): void
}

function applyAttribute(on: boolean): void {
  const root = globalThis.document?.documentElement
  if (!root) return
  if (on) root.removeAttribute('data-animations')
  else root.setAttribute('data-animations', 'off')
}

export const useAnimations = create<AnimationsStore>()((set) => ({
  // `globalThis.` so the module loads under vitest's node environment.
  enabled: globalThis.localStorage?.getItem(KEY) !== 'false',
  setEnabled(on) {
    globalThis.localStorage?.setItem(KEY, String(on))
    applyAttribute(on)
    set({ enabled: on })
  },
}))

/** Apply the stored animations preference to `<html>` at app boot. */
export function initAnimations(): void {
  applyAttribute(useAnimations.getState().enabled)
}

/** True when both the user's Appearance › Animations switch is on and the OS has not enabled Reduce Motion. */
export function motionAllowed(): boolean {
  if (!useAnimations.getState().enabled) return false
  if (typeof window !== 'undefined' && window.matchMedia?.('(prefers-reduced-motion: reduce)').matches) return false
  return true
}
