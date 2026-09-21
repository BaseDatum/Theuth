import { toast } from 'sonner'
import { ChordInput, Empty, Row, Section } from '@/components/settings/shared'
import { touch } from '@/lib/hotkeys'
import { label } from '@/lib/chord'
import { DEFS, IDS, type ShortcutId, fallback, probeShortcut, useShortcuts } from '@/lib/shortcuts'

const GROUPS: [string, ShortcutId[]][] = [
  ['Keyboard shortcuts', ['new-chat', 'search', 'toggle-sidebar', 'focus-composer', 'copy-last-response', 'copy-last-code', 'shortcuts', 'dictate']],
  ['Composer', ['send', 'interject', 'newline', 'withdraw', 'stop']],
  ['System-wide', ['toggle-window', 'cancel-all']],
]

/** Every binding, rebindable: click the chord, press keys; Backspace restores the default (`lib/shortcuts.ts`). */
export function Shortcuts() {
  const chords = useShortcuts((s) => s.chords)
  const set = useShortcuts((s) => s.set)
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
    if (DEFS[id].scope === 'global' && chord !== fallback(id)) await probeShortcut(chord)
    set(id, chord)
    toast(`${DEFS[id].label}: ${label(chord)}`)
  }
  return (
    <>
      {GROUPS.map(([title, ids]) => (
        <Section key={title} title={title}>
          {ids.map((id) => (
            <Row key={id} label={DEFS[id].label}>
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
    </>
  )
}
