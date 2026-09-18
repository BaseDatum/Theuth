import { Empty, Row, Section } from '@/components/settings/shared'
import { modKey, touch } from '@/lib/hotkeys'

/** ChatGPT's bindings (25 §2.4) plus the window toggle, as Apple key tokens; `label` spells them for the platform. */
const SHORTCUTS: [string, string[]][] = [
  ['New chat', ['⇧', '⌘', 'O']],
  ['Search', ['⌘', 'K']],
  ['Toggle sidebar', ['⇧', '⌘', 'S']],
  ['Focus composer', ['⇧', 'Esc']],
  ['Copy last response', ['⇧', '⌘', 'C']],
  ['Copy last code block', ['⇧', '⌘', ';']],
  ['Show shortcuts', ['⌘', '/']],
  ['Send', ['⏎']],
  ['New line', ['⇧', '⏎']],
  ['Withdraw last queued message', ['⌥', '↑']],
  ['Stop the running turn', ['Esc']],
  ['Show or hide the window', ['⌥', 'Space']],
  ['Cancel every running session', ['⌥', 'Esc']],
]

const PC: Record<string, string> = {
  '⌘': 'Ctrl',
  '⇧': 'Shift',
  '⌥': 'Alt',
  '⏎': 'Enter',
}

const label = (keys: string[]) =>
  modKey() === '⌘' ? keys.join('') : keys.map((k) => PC[k] ?? k).join('+')

export function Shortcuts() {
  // Touch keeps only Mod+K for a hardware keyboard (49 §5.3, `useHotkeys`).
  if (touch()) {
    return (
      <Section title="Keyboard shortcuts">
        <Empty>
          Keyboard shortcuts are not available on touch devices; with a hardware keyboard,{' '}
          {label(['⌘', 'K'])} still opens Search
        </Empty>
      </Section>
    )
  }
  return (
    <Section title="Keyboard shortcuts">
      {SHORTCUTS.map(([what, keys]) => (
        <Row key={what} label={what}>
          <kbd className="rounded-md border px-2 py-0.5 font-sans text-sm text-fg-secondary">
            {label(keys)}
          </kbd>
        </Row>
      ))}
    </Section>
  )
}
