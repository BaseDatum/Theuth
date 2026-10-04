// 96 §5.4: Settings › Voice key — the key on/off, whether the screen travels with the words, the accessibility
// reading, the first-use hints, and the disclosure (52 §8). All desktop-local (`lib/voice-key.ts`); the chords
// live in Settings › Shortcuts and the permission states in Settings › Computer.

import { Link } from '@tanstack/react-router'
import { Row, Section } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { label } from '@/lib/chord'
import { useShortcuts } from '@/lib/shortcuts'
import { useVoiceKeySettings } from '@/lib/voice-key'

export function VoiceKey() {
  const s = useVoiceKeySettings()
  const chord = useShortcuts((st) => st.chords['voice-key'])
  const dictate = useShortcuts((st) => st.chords.dictate)
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
          description="Name the links, buttons and text under the pointer from the accessibility tree (needs the Accessibility permission; its state is in Settings › Computer)."
        >
          <Switch
            checked={s.accessibility !== 'off'}
            disabled={!s.enabled || !s.includeScreen}
            onCheckedChange={(on) => s.set({ accessibility: on ? 'auto' : 'off' })}
          />
        </Row>
        <Row label="Hints" description="“Hold to talk · double-tap to lock” shows on the first three uses.">
          <Button variant="outline" size="sm" disabled={s.hintsShown === 0} onClick={() => s.set({ hintsShown: 0 })}>
            Reset hints
          </Button>
        </Row>
        <Row label="Keys" description={`Voice key ${label(chord)} · Dictate ${label(dictate)}`}>
          <Button asChild variant="outline" size="sm">
            <Link to="/settings/$tab" params={{ tab: 'shortcuts' }} search={{}}>
              Change the keys
            </Link>
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
