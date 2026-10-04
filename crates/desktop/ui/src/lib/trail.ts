// 96 §0.5: the trail is theuth's green — `--status-success-fg` of the active theme. Shared by the overlay canvas
// and the panel's `screen_context` request so the stamped image matches the screen. Kept import-free: the overlay
// webview boots with nothing but this and the Tauri listeners.

export const TRAIL_FALLBACK = '#2f5d4a'

/** `--status-success-fg` as `#rrggbb` (the stylesheet writes hex; anything else falls back). */
export function trailColor(): string {
  const v = getComputedStyle(document.documentElement).getPropertyValue('--status-success-fg').trim()
  return /^#[0-9a-f]{6}$/i.test(v) ? v : TRAIL_FALLBACK
}
