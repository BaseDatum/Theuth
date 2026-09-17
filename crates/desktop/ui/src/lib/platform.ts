/** Where the UI runs (31 §7.7): the macOS shell, the iOS shell, or a browser served by the relay. */
export type Container = 'macos' | 'ios' | 'web'

export function container(): Container {
  if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return 'web'
  // iPadOS sends a Macintosh UA; only the touch points tell it from a Mac.
  return /iPhone|iPad/.test(navigator.userAgent) || navigator.maxTouchPoints > 1 ? 'ios' : 'macos'
}

/** Coarse pointer: fingers, not a mouse. */
export const touch = (): boolean => typeof matchMedia === 'function' && matchMedia('(pointer: coarse)').matches

/** Default device name for pairing: model on iOS, "Mac" (the webview cannot see the hostname), else the browser. */
export function deviceName(): string {
  const c = container()
  if (c === 'ios') return /iPhone/.test(navigator.userAgent) ? 'iPhone' : 'iPad'
  if (c === 'macos') return 'Mac'
  const ua = typeof navigator === 'undefined' ? '' : navigator.userAgent
  for (const b of ['Firefox', 'Edg', 'Chrome', 'Safari']) if (ua.includes(b)) return b === 'Edg' ? 'Edge' : b
  return 'Browser'
}
