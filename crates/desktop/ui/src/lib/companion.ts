// The nested macOS companion (130 D8–D10; `crates/desktop/src/companion/`). While it is alive it owns
// notifications and the menu bar, so the webview suppresses `notifyIfHidden` and the tray `●N` title.
// Off Tauri (and on Windows/Linux) it is never alive and the listener is a no-op.

import { invoke } from '@tauri-apps/api/core'
import { type UnlistenFn, listen } from '@tauri-apps/api/event'
import { create } from 'zustand'

import { isTauri } from './tauri'

/** `companion-alive { alive }`: on every launch/exit of the companion and once at start. Returns the unlisten. */
export const onCompanionAlive = (cb: (alive: boolean) => void): (() => void) => {
  if (!isTauri()) return () => {}
  let unlisten: UnlistenFn | undefined
  let gone = false
  void listen<{ alive: boolean }>('companion-alive', (e) => cb(e.payload.alive)).then((u) => {
    if (gone) u()
    else unlisten = u
  })
  return () => {
    gone = true
    unlisten?.()
  }
}

/** The shell's current read; `onCompanionAlive` carries the changes. */
export const companionAlive = (): Promise<boolean> =>
  isTauri() ? invoke<boolean>('companion_alive') : Promise.resolve(false)

/**
 * 149 D15/D22: the shell's read as reactive state — while the companion is alive it owns the voice key, the
 * island and the meeting hotkeys, and the webview's copies stand down. Each webview wires itself on the first
 * render that asks; off Tauri it stays `false`.
 */
const alive = create<boolean>(() => false)
let watching = false
export const useCompanionAlive = (): boolean => {
  if (!watching && isTauri()) {
    watching = true
    void companionAlive().then((a) => alive.setState(a))
    onCompanionAlive((a) => alive.setState(a))
  }
  return alive()
}
