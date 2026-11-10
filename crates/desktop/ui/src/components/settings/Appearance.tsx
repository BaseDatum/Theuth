import { useState } from 'react'
import { Row, Section } from '@/components/settings/shared'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { useBoards } from '@/lib/boards'
import { useNudges } from '@/lib/nudges'
import { type Theme, currentTheme, setTheme } from '@/lib/theme'
import { useToolRuns } from '@/lib/tool-runs'

const THEMES: [Theme, string][] = [
  ['system', 'System'],
  ['light', 'Light'],
  ['dark', 'Dark'],
]

/** 188 A1: Settings › General › Appearance — how the app and the chat transcript look (client-only preferences). */
export function Appearance() {
  const [theme, setThemeState] = useState(currentTheme)
  const serif = useBoards((s) => s.serif)
  const collapse = useToolRuns((s) => s.collapse)
  const showNudges = useNudges((s) => s.show)
  return (
    <>
      <Section title="Interface">
        <Row label="Theme">
          <Select
            value={theme}
            onValueChange={(v) => {
              setTheme(v as Theme)
              setThemeState(v as Theme)
            }}
          >
            <SelectTrigger className="w-40">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {THEMES.map(([id, label]) => (
                <SelectItem key={id} value={id}>
                  {label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Row>
        <Row label="Serif prose on boards" description="Board prose in Newsreader; off uses the interface face. Paths, captions and gutters stay mono.">
          <Switch checked={serif} onCheckedChange={useBoards.getState().setSerif} />
        </Row>
      </Section>
      <Section title="Chat">
        <Row
          label="Collapse tool calls by default"
          description="Consecutive tool calls in a reply fold into one card with a live status line; click it to see every call. Off shows each call as its own card."
        >
          <Switch checked={collapse} onCheckedChange={useToolRuns.getState().setCollapse} />
        </Row>
        <Row
          label="Show theuth nudges"
          description="Show theuth's mid-turn notes to the model (continue, tool rounds, context pressure) in the chat. Display only."
        >
          <Switch checked={showNudges} onCheckedChange={useNudges.getState().setShow} />
        </Row>
      </Section>
    </>
  )
}
