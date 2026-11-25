// 218: the JS side of `tauri-plugin-ink`, the iOS shell's two stateless services the web cannot provide: on-device
// handwriting recognition (Vision, R6) and the Apple Pencil double-tap / squeeze forwarder. Ink itself is web-only
// (Pointer Events + perfect-freehand on every platform). Every `plugin:ink|…` invoke lives here so tests mock one module.
// The plugin exists only in the iOS shell; every failure is swallowed: a missing plugin, a desktop build or a browser all
// mean "not available", never an exception.

import type * as TauriCore from '@tauri-apps/api/core'
import { isTauri } from '@/lib/tauri'

export type PencilAction = 'switchEraser' | 'switchPrevious' | 'showColorPalette' | 'showInkAttributes' | 'showContextualPalette' | 'runSystemShortcut' | 'ignore'
/** A Pencil double-tap or squeeze, with the action the owner prefers in Settings → Apple Pencil. */
export type PencilEvent = { kind: 'pencil'; gesture: 'tap' | 'squeeze'; action: PencilAction }
type PluginEvent = PencilEvent | { kind: 'error'; message: string }
type Core = typeof TauriCore

/** Overridable for tests: the `@tauri-apps/api/core` loader (null = not available). */
export const loader: { core: () => Promise<Core | null>; tauri: () => boolean } = {
  core: () => import('@tauri-apps/api/core').catch(() => null),
  tauri: () => typeof window !== 'undefined' && isTauri(),
}

async function core(): Promise<Core | null> {
  if (!loader.tauri()) return null
  return loader.core()
}

/** Vision handwriting text of a mark's PNG (base64). null outside the iOS shell or on any failure. */
export async function recognizeText(png: string): Promise<{ text: string; confidence: number } | null> {
  try {
    const c = await core()
    if (!c) return null
    const r = await c.invoke<{ text: string; confidence: number } | null>('plugin:ink|recognize_text', { png })
    return r && typeof r.text === 'string' ? r : null
  } catch {
    return null
  }
}

const noop = () => {}
/** The plugin keeps one listener (a second `pencil_listen` replaces the channel): only the latest may unlisten. */
let generation = 0

/**
 * Forward Pencil double-taps and squeezes to `on` until the returned function is called. Resolves to a no-op where the
 * plugin is absent or reports no Pencil interaction (desktop, browser, iPhone, iPadOS below 17.5).
 */
export async function listenPencil(on: (e: PencilEvent) => void): Promise<() => void> {
  try {
    const c = await core()
    if (!c) return noop
    const onEvent = new c.Channel<PluginEvent>()
    onEvent.onmessage = (e) => {
      if (e.kind === 'pencil') try { on(e) } catch { /* a handler never breaks the channel */ }
    }
    const mine = ++generation
    const r = await c.invoke<{ ok: boolean }>('plugin:ink|pencil_listen', { onEvent })
    if (!r?.ok) return noop
    let done = false
    return () => {
      if (done) return
      done = true
      if (mine === generation) void c.invoke('plugin:ink|pencil_unlisten', {}).catch(noop)
    }
  } catch {
    return noop
  }
}
