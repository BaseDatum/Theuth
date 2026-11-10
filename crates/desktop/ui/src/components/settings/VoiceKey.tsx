// 96 §5.4: Settings › Voice key — the key on/off, whether the screen travels with the words, the accessibility
// reading, when the trail is drawn, where the panel sits, the first-use hints, and the disclosure (52 §8). All
// desktop-local (`lib/voice-key.ts`); the chords live only in General › Shortcuts (188: the Keys summary row went)
// and the permission states in Tools › Computer use.

import { useEffect, useRef, useState } from 'react'
import { Row, Section } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { label } from '@/lib/chord'
import { useShortcuts } from '@/lib/shortcuts'
import { type Annotate, onVoiceKeyPlaced, voiceKeyPlace } from '@/lib/tauri'
import { useVoiceKeySettings } from '@/lib/voice-key'

export function VoiceKey() {
  const s = useVoiceKeySettings()
  const chord = useShortcuts((st) => st.chords['voice-key'])
  // The island answers `voice-key:placed` when its Done is pressed or a gesture starts; leaving the tab hides a panel it showed.
  const [placing, setPlacing] = useState(false)
  const shown = useRef(false)
  shown.current = placing
  useEffect(() => {
    const off = onVoiceKeyPlaced(() => setPlacing(false))
    return () => {
      void off.then((f) => f())
      if (shown.current) void voiceKeyPlace({ on: false })
    }
  }, [])
  return (
    <>
      <Section title="Voice key">
        <Row
          label="Voice key"
          description={`Hold ${label(chord)} anywhere to ask theuth about what is on your screen; release to send, double-tap to lock hands-free. The answer is spoken.`}
        >
          <Switch checked={s.enabled} onCheckedChange={(on) => s.set({ enabled: on })} />
        </Row>
        <Row
          label="Send the screen"
          description="Capture the displays the pointer visited and stamp the path onto them; off, only your words go to theuth."
        >
          <Switch checked={s.includeScreen} disabled={!s.enabled} onCheckedChange={(on) => s.set({ includeScreen: on })} />
        </Row>
        <Row
          label="Read what the pointer touched"
          description="Name the links, buttons and text under the pointer from the accessibility tree (needs the Accessibility permission; its state is in Tools › Computer use)."
        >
          <Switch
            checked={s.accessibility !== 'off'}
            disabled={!s.enabled || !s.includeScreen}
            onCheckedChange={(on) => s.set({ accessibility: on ? 'auto' : 'off' })}
          />
        </Row>
        <Row
          label="Draw the trail"
          description="With the mouse option, clicks go to the trail while the key is held, not to the app underneath."
        >
          <Select value={s.annotate} disabled={!s.enabled} onValueChange={(v) => s.set({ annotate: v as Annotate })}>
            <SelectTrigger className="max-w-72" aria-label="Draw the trail">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="always">Always while the key is held</SelectItem>
              <SelectItem value="mouse">Only while the left mouse button is held</SelectItem>
            </SelectContent>
          </Select>
        </Row>
        <Row label="Panel position" description="The voice panel and the meeting island share this spot (saved per display layout).">
          <Button
            variant="outline"
            size="sm"
            onClick={() => {
              setPlacing(!placing)
              void voiceKeyPlace({ on: !placing })
            }}
          >
            {placing ? 'Hide panel' : 'Show panel to move it'}
          </Button>
          <Button variant="outline" size="sm" onClick={() => void voiceKeyPlace({ on: placing, reset: true })}>
            Reset to default
          </Button>
        </Row>
        <Row label="Hints" description="“Hold to talk · double-tap to lock” shows on the first three uses.">
          <Button variant="outline" size="sm" disabled={s.hintsShown === 0} onClick={() => s.set({ hintsShown: 0 })}>
            Reset hints
          </Button>
        </Row>
      </Section>
      <p className="text-[13px] text-fg-tertiary">
        While the voice key is held, theuth captures the displays the pointer visits and sends them to the model with your
        words; screenshots may contain private information.
      </p>
    </>
  )
}
