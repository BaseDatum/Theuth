// Typed wrappers for the Tauri commands and events of `crates/desktop/src/commands.rs`
// and `main.rs`. Outside Tauri (`bun run dev` in a browser) the connection comes
// from `VITE_THEUTH_URL` / `VITE_THEUTH_TOKEN` instead, so the UI can be worked on
// against a terminal `t --serve` without the shell.

import { invoke } from '@tauri-apps/api/core'
import { type UnlistenFn, listen } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'

/** `launch::Connection`: what the webview needs to open the WebSocket. */
export type Connection = {
  url: string
  token: string
  data_dir: string
  owned: boolean
}

export const isTauri = () => '__TAURI_INTERNALS__' in window

/** The island webview (53 decision 8, 03 §0.1): the same bundle routed by window label; `/island` in a plain browser (the fixture). */
export const isIsland = (): boolean =>
  isTauri() ? getCurrentWindow().label === 'island' : location.pathname === '/island'

const devConnection = (): Connection => {
  const url = import.meta.env.VITE_THEUTH_URL
  const token = import.meta.env.VITE_THEUTH_TOKEN
  if (typeof url !== 'string' || typeof token !== 'string') {
    throw new Error(
      'Not running in the app; set VITE_THEUTH_URL and VITE_THEUTH_TOKEN to connect to a daemon',
    )
  }
  return { url, token, data_dir: '', owned: false }
}

/** Probe, else spawn the daemon on a pty and wait for its port; cached after the first success. */
export const connection = (): Promise<Connection> =>
  isTauri() ? invoke<Connection>('connection') : Promise.resolve(devConnection())

/** The pty transcript so far; `onLaunchLog` carries the increments. */
export const launchLog = (): Promise<string> =>
  isTauri() ? invoke<string>('launch_log') : Promise.resolve('')

/** Stop the daemon we own (if any) and connect again. */
export const relaunch = (): Promise<Connection> =>
  isTauri() ? invoke<Connection>('relaunch') : Promise.resolve(devConnection())

/** The webview finished its part of the quit sequence; the app exits now. */
export const quitAck = (): Promise<void> =>
  isTauri() ? invoke<void>('quit_ack') : Promise.resolve()

const on = <T>(event: string, cb: (payload: T) => void): Promise<UnlistenFn> =>
  isTauri() ? listen<T>(event, (e) => cb(e.payload)) : Promise.resolve(() => {})

export const onLaunchLog = (cb: (chunk: string) => void) => on<string>('launch-log', cb)
/** Tray menu / app menu: show the window and go to `path` (`/`, `/knowledge`, `/settings/general`). */
export const onNavigate = (cb: (path: string) => void) => on<string>('navigate', cb)
/** ⌘Q / tray Quit: run RPC `shutdown` if we own the daemon, then `quitAck`. */
export const onQuitRequested = (cb: () => void) => on<null>('quit-requested', cb)
/** 39 §3.3: the app downloaded and verified a newer bundle; it installs at quit. */
export const onUpdateReady = (cb: (p: { version: string }) => void) =>
  on<{ version: string }>('update-ready', cb)
/** Settings › General: check now; the parked version, or `null` when current. */
export const checkUpdate = (): Promise<string | null> =>
  isTauri() ? invoke<string | null>('check_update') : Promise.resolve(null)
/** Settings › General: run the quit handshake and relaunch into the parked bundle. */
export const installUpdate = (): Promise<void> =>
  isTauri() ? invoke<void>('install_update') : Promise.resolve()
/** `crates/desktop/src/models.rs`: model files fetched from the release bucket at start; `verifying` = hashing a pre-existing file once. */
export type ModelsProgress =
  | { state: 'downloading'; file: string; done: number; total: number }
  | { state: 'verifying'; file: string }
  | { state: 'ready' }
  | { state: 'failed'; message: string }
export const onModelsProgress = (cb: (p: ModelsProgress) => void) =>
  on<ModelsProgress>('models-progress', cb)

// Meetings island (`crates/desktop/src/island.rs`, 03 §1). No-ops outside Tauri.
/** `set_content_protected` on both windows (decision 9); every webview then hears `capture-hidden`. */
export const setHiddenFromCapture = (hidden: boolean): Promise<void> =>
  isTauri() ? invoke<void>('set_hidden_from_capture', { hidden }) : Promise.resolve()
/** Order the island front without focusing anything, sized `width × height` logical px (44 high = strip only; the shell clamps). */
export const islandShow = (width: number, height: number): Promise<void> =>
  isTauri() ? invoke<void>('island_show', { width, height }) : Promise.resolve()
export const islandHide = (): Promise<void> => (isTauri() ? invoke<void>('island_hide') : Promise.resolve())
/** The request box was left: the panel resigns key so keystrokes return to the meeting app. */
export const islandResignKey = (): Promise<void> =>
  isTauri() ? invoke<void>('island_resign_key') : Promise.resolve()
/** Place the island: `positions[key]` for the current screen configuration wins, else the notch default. Answers `[x, y, key]`. */
export const islandPlace = (positions: Record<string, [number, number]>): Promise<[number, number, string]> =>
  isTauri() ? invoke<[number, number, string]>('island_place', { positions }) : Promise.resolve([0, 0, 'browser'])
export const onCaptureHidden = (cb: (hidden: boolean) => void) => on<boolean>('capture-hidden', cb)
/** `NSApplicationDidChangeScreenParametersNotification`: re-place the island. */
export const onScreensChanged = (cb: () => void) => on<null>('screens-changed', cb)
/** 58 §6.2 / §3.3: what this OS's island can do. Off Tauri: everything, so the fixture shows the full UI. */
export type IslandCapabilities = {
  hide_from_share: boolean
  always_on_top: boolean
  non_activating: boolean
  placement: 'notch' | 'top-centre' | 'user'
}
export const islandCapabilities = (): Promise<IslandCapabilities> =>
  isTauri()
    ? invoke<IslandCapabilities>('island_capabilities')
    : Promise.resolve({ hide_from_share: true, always_on_top: true, non_activating: true, placement: 'notch' })
