// Typed wrappers for the Tauri commands and events of `crates/desktop/src/commands.rs`
// and `main.rs`. Outside Tauri (`bun run dev` in a browser) the connection comes
// from `VITE_WTH_URL` / `VITE_WTH_TOKEN` instead, so the UI can be worked on
// against a terminal `wth --serve` without the shell.

import { invoke } from '@tauri-apps/api/core'
import { type UnlistenFn, listen } from '@tauri-apps/api/event'

/** `launch::Connection`: what the webview needs to open the WebSocket. */
export type Connection = {
  url: string
  token: string
  data_dir: string
  owned: boolean
}

export const isTauri = () => '__TAURI_INTERNALS__' in window

const devConnection = (): Connection => {
  const url = import.meta.env.VITE_WTH_URL
  const token = import.meta.env.VITE_WTH_TOKEN
  if (typeof url !== 'string' || typeof token !== 'string') {
    throw new Error(
      'Not running in the app; set VITE_WTH_URL and VITE_WTH_TOKEN to connect to a daemon',
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
