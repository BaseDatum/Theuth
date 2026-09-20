// 53 decisions 9–10 / 03 §3: Settings › Meetings — the eye (hidden from capture) with its hotkey, the
// push-to-talk hotkey, and the black-screenshot note. All desktop-local (`lib/meeting-settings.ts`).

import { useState } from 'react'
import { toast } from 'sonner'
import { Row, Section } from '@/components/settings/shared'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { chordOf, probeShortcut } from '@/lib/meeting-hotkeys'
import { DEFAULT_EYE, DEFAULT_PTT, useMeetingSettings } from '@/lib/meeting-settings'
import { isMacDesktop } from '@/lib/hotkeys'
import { setHiddenFromCapture } from '@/lib/tauri'

export function Meetings() {
  const hidden = useMeetingSettings((s) => s.hidden)
  const ptt = useMeetingSettings((s) => s.ptt)
  const eye = useMeetingSettings((s) => s.eye)
  const local = isMacDesktop()
  return (
    <>
      <Section title="Screen capture">
        <Row
          label="Hidden from capture"
          description="Both windows are excluded from screen sharing and screenshots (NSWindow sharingType none). Never automatic: the harness does not detect screen sharing. Your own ⌘⇧3/4 screenshots of the windows come out black while this is on."
        >
          <Switch checked={hidden} disabled={!local} onCheckedChange={(on) => void setHiddenFromCapture(on)} />
        </Row>
        <Row label="Eye hotkey" description="Flips hidden from capture while a meeting records. Click the field and press a chord.">
          <Hotkey kind="eye" value={eye} fallback={DEFAULT_EYE} />
        </Row>
      </Section>
      <Section title="Push-to-talk">
        <Row
          label="Push-to-talk hotkey"
          description="Hold to ask the meeting session a question; what you say while it is held is tagged as a request and answered as a brief. Registered only while a meeting records or is paused."
        >
          <Hotkey kind="ptt" value={ptt} fallback={DEFAULT_PTT} />
        </Row>
      </Section>
    </>
  )
}

/** A chord reader: focus, press the chord, done. A conflicting chord (register throws) toasts and keeps the old one. */
function Hotkey({ kind, value, fallback }: { kind: 'ptt' | 'eye'; value: string; fallback: string }) {
  const setHotkey = useMeetingSettings((s) => s.setHotkey)
  const [reading, setReading] = useState(false)
  return (
    <Input
      readOnly
      className="w-40 text-center font-mono"
      value={reading ? 'press a chord…' : value}
      onFocus={() => setReading(true)}
      onBlur={() => setReading(false)}
      onKeyDown={(e) => {
        e.preventDefault()
        if (e.key === 'Escape') return e.currentTarget.blur()
        if (e.key === 'Backspace') {
          setHotkey(kind, fallback)
          return e.currentTarget.blur()
        }
        const chord = chordOf(e.nativeEvent)
        if (!chord) return
        const el = e.currentTarget
        if (chord === value) return el.blur()
        void probeShortcut(chord)
          .then(() => setHotkey(kind, chord))
          .catch(() => toast.error(`Could not register ${chord}; another app holds it`))
          .finally(() => el.blur())
      }}
    />
  )
}
