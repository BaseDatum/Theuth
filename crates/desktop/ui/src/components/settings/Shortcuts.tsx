import { notify as toast } from '@/lib/notify'
import { ChordInput, Empty, Row, Section } from '@/components/settings/shared'
import { touch } from '@/lib/hotkeys'
import { label } from '@/lib/chord'
import { DEFAULT_EYE, DEFAULT_PTT, useMeetingSettings } from '@/lib/meeting-settings'
import { DEFS, IDS, type ShortcutId, fallback, probeShortcut, useShortcuts } from '@/lib/shortcuts'
import { voiceKeyBind } from '@/lib/tauri'
import { useVoiceKeySettings } from '@/lib/voice-key'

const GROUPS: [string, ShortcutId[]][] = [
  ['Keyboard shortcuts', ['new-chat', 'search', 'toggle-sidebar', 'focus-composer', 'copy-last-response', 'copy-last-code', 'shortcuts', 'dictate', 'voice-key']],
  ['Composer', ['send', 'interject', 'newline', 'withdraw', 'stop']],
  ['System-wide', ['toggle-window', 'cancel-all']],
]

/** 96 §12: a hint, not a blocker — a Carbon hot key swallows its chord in every app while registered. */
const TERMINAL_KEYS = ['Control+D', 'Control+C', 'Control+Z']

/** Every binding, rebindable: click the chord, press keys; Backspace restores the default (`lib/shortcuts.ts`). */
export function Shortcuts() {
  const chords = useShortcuts((s) => s.chords)
  const set = useShortcuts((s) => s.set)
  const eye = useMeetingSettings((s) => s.eye)
  const ptt = useMeetingSettings((s) => s.ptt)
  // Touch keeps only Mod+K for a hardware keyboard (49 §5.3, `useHotkeys`).
  if (touch()) {
    return (
      <Section title="Keyboard shortcuts">
        <Empty>
          Keyboard shortcuts are not available on touch devices; with a hardware keyboard, {label(chords.search)} still opens
          Search
        </Empty>
      </Section>
    )
  }
  const commit = async (id: ShortcutId, chord: string) => {
    const taken = IDS.find((o) => o !== id && chords[o] === chord && DEFS[o].scope === DEFS[id].scope)
    if (taken) throw new Error(`${label(chord)} is already bound to ${DEFS[taken].label}`)
    // 96 §4.3: the voice key is the shell's registration — it binds (or rejects, keeping the old chord) before the store moves.
    if (id === 'voice-key') {
      if (useVoiceKeySettings.getState().enabled) await voiceKeyBind(chord)
    } else if (DEFS[id].scope === 'global' && chord !== fallback(id)) await probeShortcut(chord)
    set(id, chord)
    toast(`${DEFS[id].label}: ${label(chord)}`)
  }
  return (
    <>
      {GROUPS.map(([title, ids]) => (
        <Section key={title} title={title}>
          {ids.map((id) => (
            <Row
              key={id}
              label={DEFS[id].label}
              description={id === 'voice-key' && TERMINAL_KEYS.includes(chords[id]) ? 'Terminals use this key; theuth will take it while running' : undefined}
            >
              <ChordInput
                value={chords[id]}
                fallback={fallback(id)}
                bare={DEFS[id].scope === 'composer'}
                display={label}
                commit={(chord) => commit(id, chord)}
              />
            </Row>
          ))}
        </Section>
      ))}
      {/* 188: the meeting hotkeys, moved from Settings › Meetings; they live in `lib/meeting-settings.ts`, not `lib/shortcuts.ts`. */}
      <Section title="Meetings">
        <Row label="Eye hotkey" description="Flips hidden from capture while a meeting records. Click the field and press a chord.">
          <MeetingHotkey kind="eye" value={eye} fallback={DEFAULT_EYE} />
        </Row>
        <Row
          label="Push-to-talk hotkey"
          description="Hold to ask the meeting session a question; what you say while it is held is tagged as a request and answered as a brief. Registered only while a meeting records or is paused."
        >
          <MeetingHotkey kind="ptt" value={ptt} fallback={DEFAULT_PTT} />
        </Row>
      </Section>
    </>
  )
}

/** A meeting global shortcut: probed with the plugin (a conflict rejects) before it is saved. */
function MeetingHotkey({ kind, value, fallback }: { kind: 'ptt' | 'eye'; value: string; fallback: string }) {
  const setHotkey = useMeetingSettings((s) => s.setHotkey)
  return (
    <ChordInput
      value={value}
      fallback={fallback}
      display={label}
      commit={(chord) => (chord === fallback ? setHotkey(kind, chord) : probeShortcut(chord).then(() => setHotkey(kind, chord)))}
    />
  )
}
