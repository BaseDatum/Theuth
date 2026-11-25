// 96 §0.5: the trail is theuth's green — `--status-success-fg` of the active theme. Shared by the overlay canvas
// and the panel's `screen_context` request so the stamped image matches the screen. Kept import-free: the overlay
// webview boots with nothing but this and the Tauri listeners (so the trail mode is read straight from storage).

export const TRAIL_FALLBACK = '#265c47'

/** `--status-success-fg` as `#rrggbb` (the stylesheet writes hex; anything else falls back). */
export function trailColor(): string {
  const v = getComputedStyle(document.documentElement).getPropertyValue('--status-success-fg').trim()
  return /^#[0-9a-f]{6}$/i.test(v) ? v : TRAIL_FALLBACK
}

/** Settings › Voice key "Draw the trail" as `lib/voice-key.ts` persists it (`voiceKey.annotate`); `always` unless `mouse`. */
export function annotateMode(): 'always' | 'mouse' {
  try {
    const saved: unknown = JSON.parse(localStorage.getItem('voiceKey') ?? '{}')
    return saved && typeof saved === 'object' && 'annotate' in saved && saved.annotate === 'mouse' ? 'mouse' : 'always'
  } catch {
    return 'always'
  }
}
