import { Row, Section } from '@/components/settings/shared'

/** ChatGPT's bindings (25 §2.4) plus the window toggle. */
const SHORTCUTS: [string, string][] = [
  ['New chat', '⇧⌘O'],
  ['Search chats', '⌘K'],
  ['Toggle sidebar', '⇧⌘S'],
  ['Focus composer', '⇧Esc'],
  ['Copy last response', '⇧⌘C'],
  ['Copy last code block', '⇧⌘;'],
  ['Show shortcuts', '⌘/'],
  ['Send', '⏎'],
  ['New line', '⇧⏎'],
  ['Withdraw last queued message', '⌥↑'],
  ['Stop the running turn', 'Esc'],
  ['Show or hide the window', '⌥Space'],
  ['Cancel every running session', '⌥Esc'],
]

export function Shortcuts() {
  return (
    <Section title="Keyboard shortcuts">
      {SHORTCUTS.map(([what, keys]) => (
        <Row key={what} label={what}>
          <kbd className="rounded-md border px-2 py-0.5 font-sans text-sm text-fg-secondary">{keys}</kbd>
        </Row>
      ))}
    </Section>
  )
}
