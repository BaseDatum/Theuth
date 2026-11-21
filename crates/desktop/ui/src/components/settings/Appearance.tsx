import { useState } from 'react'
import { Row, Section } from '@/components/settings/shared'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { useBoards } from '@/lib/boards'
import { useNudges } from '@/lib/nudges'
import { LINGER_MAX_HOURS, useLinger } from '@/lib/sidebar-linger'
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
      <Section title="Sidebar">
        <Row
          label="Keep recently active sessions in Active for"
          description="A chat that stops working stays at the bottom of Active this long, then files back under its project. 0 turns this off."
        >
          <LingerHours />
        </Row>
      </Section>
    </>
  )
}

/** Hours field: a draft while typing, saved on every valid number (whole or fractional, 0 – one week). */
function LingerHours() {
  const hours = useLinger((s) => s.hours)
  const [draft, setDraft] = useState<string | null>(null)
  return (
    <div className="flex items-center gap-2">
      <Input
        type="number"
        inputMode="decimal"
        min={0}
        max={LINGER_MAX_HOURS}
        step={1}
        className="w-20"
        aria-label="Hours to keep recently active sessions in Active"
        value={draft ?? String(hours)}
        onChange={(e) => {
          const v = e.target.value
          setDraft(v)
          const n = Number(v)
          if (v.trim() !== '' && Number.isFinite(n) && n >= 0) useLinger.getState().setHours(n)
        }}
        onBlur={() => setDraft(null)}
      />
      <span className="text-sm text-fg-secondary">hours</span>
    </div>
  )
}
