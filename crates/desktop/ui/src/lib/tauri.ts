// Typed wrappers for the Tauri commands and events of `crates/desktop/src/commands.rs`
// and `main.rs`. Outside Tauri (`bun run dev` in a browser) the connection comes
// from `VITE_THEUTH_URL` / `VITE_THEUTH_TOKEN` instead, so the UI can be worked on
// against a terminal `t --serve` without the shell.

import { invoke } from '@tauri-apps/api/core'
import { type UnlistenFn, emitTo, listen } from '@tauri-apps/api/event'
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
/** 96 §4.5: an overlay webview (`overlay-<n>`, one per display) — the trail canvas; `/overlay` in a plain browser. */
export const isOverlay = (): boolean =>
  isTauri() ? getCurrentWindow().label.startsWith('overlay-') : location.pathname === '/overlay'

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

/** The webview finished its part of the quit sequence; the shell's quit worker takes it from here (163). */
export const quitAck = (): Promise<void> =>
  isTauri() ? invoke<void>('quit_ack') : Promise.resolve()

const on = <T>(event: string, cb: (payload: T) => void): Promise<UnlistenFn> =>
  isTauri() ? listen<T>(event, (e) => cb(e.payload)) : Promise.resolve(() => {})

export const onLaunchLog = (cb: (chunk: string) => void) => on<string>('launch-log', cb)
/** Tray menu / app menu: show the window and go to `path` (`/`, `/knowledge`, `/settings/general`). */
export const onNavigate = (cb: (path: string) => void) => on<string>('navigate', cb)
/** `quit::Requested`: `restart` on Settings › Install and restart. */
export type QuitRequested = { restart: boolean }
/** `quit::Progress`: one per quit-worker step (163 D4); a second `engine` with `forced` when the daemon was killed. */
export type QuitProgress = { step: 'companion' | 'engine' | 'closing'; forced?: boolean }
/** ⌘Q / tray Quit / `theuth://quit` / Install and restart: show the overlay, RPC `shutdown` if we own the daemon, then `quitAck`. */
export const onQuitRequested = (cb: (p: QuitRequested) => void) => on<QuitRequested>('quit-requested', cb)
export const onQuitProgress = (cb: (p: QuitProgress) => void) => on<QuitProgress>('quit-progress', cb)
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

// Voice key (`crates/desktop/src/point/`, 96 §4.3–4.4). No-ops outside Tauri.
/** `point::machine::State` serialised (`#[serde(tag = "state")]`); times are ms since process start. */
export type PointState =
  | { state: 'idle' }
  | { state: 'armed'; t0: number }
  | { state: 'holding'; t0: number }
  | { state: 'tap1'; t0: number; released: number }
  | { state: 'locked'; t0: number }
  | { state: 'sending'; t0: number }
  | { state: 'replying' }
/** One pointer sample in logical global points, `t_ms` from the press; `pen` = drawn (false while the mouse button is up in `mouse` mode). */
export type PointSample = { t_ms: number; x: number; y: number; pen: boolean }
export type PointStart = { at_ms: number; locked: boolean; color_hint: string | null }
/** A tick's new points, local to one display; the batch shares one pen state. */
export type PointSamples = { display_index: number; pts: [number, number][]; pen: boolean }
export type PointStop = { samples: PointSample[]; duration_ms: number; reason: 'release' | 'tap' }
export type PointCancel = { reason: 'esc' | 'short' | 'double-tap-missed' | 'cancel'; hint: string | null }
export type PointRefused = { reason: string }
/** When the trail is drawn: the whole hold, or only while the left mouse button is held (the overlay then takes the clicks). */
export type Annotate = 'always' | 'mouse'
/** Settings › Voice key → the island: show the panel to drag it (`on`), hide it again, or forget the saved spot (`reset`). */
export type VoiceKeyPlace = { on: boolean; reset?: boolean }

/** Register `chord` as the voice key (`""` unregisters); rejects with a string and keeps the old chord when it cannot be taken. */
export const voiceKeyBind = (chord: string): Promise<void> =>
  isTauri() ? invoke<void>('voice_key_bind', { chord }) : Promise.resolve()
/** Esc inside the panel, a click outside, Stop, or a failed send. */
export const pointCancel = (): Promise<void> => (isTauri() ? invoke<void>('point_cancel') : Promise.resolve())
/** The panel finished a phase: `sending → replying`, and `replying → idle` after the reply. */
export const pointDone = (): Promise<void> => (isTauri() ? invoke<void>('point_done') : Promise.resolve())
export const pointState = (): Promise<PointState> =>
  isTauri() ? invoke<PointState>('point_state') : Promise.resolve({ state: 'idle' })
/** A standing refusal (`'meeting'` while one records) answered to the next press; `null` clears it. */
export const pointRefuse = (reason: string | null): Promise<void> =>
  isTauri() ? invoke<void>('point_refuse', { reason }) : Promise.resolve()
/** The trail mode (Settings › Voice key); the main webview sends it after `voice_key_bind` and on every change. */
export const pointConfigure = (annotate: Annotate): Promise<void> =>
  isTauri() ? invoke<void>('point_configure', { annotate }) : Promise.resolve()
/** Create a missing `overlay-<n>` per monitor; the labels. */
export const overlayEnsure = (): Promise<string[]> =>
  isTauri() ? invoke<string[]>('overlay_ensure') : Promise.resolve([])
export const onPointStart = (cb: (p: PointStart) => void) => on<PointStart>('point:start', cb)
export const onPointLock = (cb: () => void) => on<Record<string, never>>('point:lock', cb)
export const onPointSamples = (cb: (p: PointSamples) => void) => on<PointSamples>('point:samples', cb)
export const onPointStop = (cb: (p: PointStop) => void) => on<PointStop>('point:stop', cb)
export const onPointCancel = (cb: (p: PointCancel) => void) => on<PointCancel>('point:cancel', cb)
export const onPointRefused = (cb: (p: PointRefused) => void) => on<PointRefused>('point:refused', cb)
export const voiceKeyPlace = (p: VoiceKeyPlace): Promise<void> => (isTauri() ? emitTo('island', 'voice-key:place', p) : Promise.resolve())
export const onVoiceKeyPlace = (cb: (p: VoiceKeyPlace) => void) => on<VoiceKeyPlace>('voice-key:place', cb)
/** The island left `placing` (Done, or a gesture started): Settings' button flips back. */
export const voiceKeyPlaced = (): Promise<void> => (isTauri() ? emitTo('main', 'voice-key:placed', {}) : Promise.resolve())
export const onVoiceKeyPlaced = (cb: () => void) => on<Record<string, never>>('voice-key:placed', cb)
