// 53 decisions 9–10 / 03 §4: the two global shortcuts of a meeting — push-to-talk (held → `meeting_ptt`)
// and the eye (toggle hidden-from-capture). Registered from the main webview only, and only while a
// meeting records or is paused: a global ⌥` outside a meeting is a stolen key.

import { register, unregister } from '@tauri-apps/plugin-global-shortcut'
import { useEffect } from 'react'
import { toast } from 'sonner'
import { useMeetingSettings } from '@/lib/meeting-settings'
import { container } from '@/lib/platform'
import { isIsland, setHiddenFromCapture } from '@/lib/tauri'
import { live, useMeeting } from '@/store/meeting'
import { useConnection } from '@/store/connection'

/** `Alt+Shift+E`-style chord from a keydown; `null` for a bare modifier or a chord without one. */
export function chordOf(e: KeyboardEvent): string | null {
  const mods = [e.altKey && 'Alt', e.ctrlKey && 'Control', e.shiftKey && 'Shift', e.metaKey && 'Super'].filter(Boolean)
  if (mods.length === 0 || ['Alt', 'Control', 'Shift', 'Meta'].includes(e.key)) return null
  // `global_hotkey` reads `KeyE`/`Digit1` as `E`/`1`, and every other `KeyboardEvent.code` by name (`Backquote`, `F5`, `Space`).
  const key = e.code.replace(/^Key|^Digit/, '')
  return key ? [...mods, key].join('+') : null
}

/** Can this chord be registered right now? A conflict (another app, or our own live binding) throws. */
export async function probeShortcut(chord: string): Promise<void> {
  await register(chord, () => {})
  await unregister(chord)
}

/** Mount once in the main webview beside `useTauri()`. */
export function useMeetingHotkeys(): void {
  const active = useMeeting((s) => live(s.state))
  const ptt = useMeetingSettings((s) => s.ptt)
  const eye = useMeetingSettings((s) => s.eye)
  useEffect(() => {
    if (!active || container() !== 'macos' || isIsland()) return
    let held = false
    const api = () => useConnection.getState().api
    void register(ptt, (e) => {
      const down = e.state === 'Pressed'
      if (down === held) return
      held = down
      void api()?.meeting_ptt(down).catch(() => undefined)
    }).catch(() => toast.error(`Could not register ${ptt}; another app holds it`))
    void register(eye, (e) => {
      if (e.state === 'Pressed') void setHiddenFromCapture(!useMeetingSettings.getState().hidden)
    }).catch(() => toast.error(`Could not register ${eye}; another app holds it`))
    return () => {
      void unregister([ptt, eye]).catch(() => undefined)
      // The key may still be down across a state change or rebind: the span must close.
      if (held) void api()?.meeting_ptt(false).catch(() => undefined)
    }
  }, [active, ptt, eye])
}
