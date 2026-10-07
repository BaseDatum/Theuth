/**
 * COMPOSER-FOCUS (Sky, 2026-10-07): "when I send a chat input in a session, the focus is taken from the text input,
 * so pressing esc doesn't cancel the turn". After a send the composer takes the caret back on desktop so Esc (`keys.stop`)
 * reaches its textarea; on a phone/tablet (`touch`) never — the keyboard must not pop back up. It never steals focus
 * from a dialog or another input that opened meanwhile: only when nothing has it (`body`/`null`) or something inside
 * the composer's own `box` does (the Send button that just went `disabled`, a chip).
 */
export function shouldRefocus(active: Element | null, box: Element | null, touch: boolean, body: Element | null = globalThis.document?.body ?? null): boolean {
  if (touch) return false
  if (active === null || active === body) return true
  return box !== null && box.contains(active)
}
