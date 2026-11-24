import { openUrl } from '@tauri-apps/plugin-opener'

/** Where the UI runs (31 §7.7): a desktop shell (macOS, Linux, Windows), the iOS shell, or a browser served by the relay. */
export type Container = 'macos' | 'linux' | 'windows' | 'ios' | 'web'

export function container(): Container {
  if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return 'web'
  const ua = navigator.userAgent
  if (/iPhone|iPad/.test(ua)) return 'ios'
  // The UA decides Windows and Linux BEFORE the touch test: touchscreen laptops and RDP sessions with touch
  // redirection report touch points too (build-B over RDP: 10), and an 'ios' verdict skips the local engine.
  // WebView2: "Windows NT 10.0; Win64; x64".
  if (/Windows NT/.test(ua)) return 'windows'
  // WebKitGTK: "X11; Linux x86_64" (also under Wayland).
  if (/Linux/.test(ua)) return 'linux'
  // iPadOS sends a Macintosh UA; only the touch points tell it from a Mac.
  return navigator.maxTouchPoints > 1 ? 'ios' : 'macos'
}

/** Marks `<html data-container>` once at boot, before the first render, so CSS can key on the shell (the `ios:` variant). */
export function markContainer(): void {
  document.documentElement.dataset.container = container()
}

/** A desktop shell with its own daemon beside it (macOS, Linux, Windows): host paths, native drop, autostart, tray, global shortcuts. */
export const isDesktop = (): boolean => {
  const c = container()
  return c === 'macos' || c === 'linux' || c === 'windows'
}

/** Coarse pointer: fingers, not a mouse. */
export const touch = (): boolean => typeof matchMedia === 'function' && matchMedia('(pointer: coarse)').matches

/** Opens a link the way chat links open: the OS browser under Tauri, a new tab where `openUrl` is unavailable (BoardReader's fallback). */
export const openExternal = (href: string) => void openUrl(href).catch(() => window.open(href, '_blank', 'noopener'))

/** Default device name for pairing: model on iOS, "Mac"/"Linux"/"Windows" (the webview cannot see the hostname), else the browser. */
export function deviceName(): string {
  const c = container()
  if (c === 'ios') return /iPhone/.test(navigator.userAgent) ? 'iPhone' : 'iPad'
  if (c === 'macos') return 'Mac'
  if (c === 'linux') return 'Linux'
  if (c === 'windows') return 'Windows'
  const ua = typeof navigator === 'undefined' ? '' : navigator.userAgent
  for (const b of ['Firefox', 'Edg', 'Chrome', 'Safari']) if (ua.includes(b)) return b === 'Edg' ? 'Edge' : b
  return 'Browser'
}
