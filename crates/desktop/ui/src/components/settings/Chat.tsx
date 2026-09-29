import { Row, Section } from '@/components/settings/shared'
import { Switch } from '@/components/ui/switch'
import { useTalkSettings } from '@/lib/talk-settings'
import { useToolRuns } from '@/lib/tool-runs'

/** Settings › Chat: how the transcript displays, and the talk view's cue. Client-only preferences; nothing here changes what the engine sends. */
export function Chat() {
  const collapse = useToolRuns((s) => s.collapse)
  const cue = useTalkSettings((s) => s.cue)
  return (
    <>
      <Section title="Tool calls">
        <Row
          label="Collapse tool calls by default"
          description="Consecutive tool calls in a reply fold into one card with a live status line; click it to see every call. Off shows each call as its own card."
        >
          <Switch checked={collapse} onCheckedChange={useToolRuns.getState().setCollapse} />
        </Row>
      </Section>
      <Section title="Talk">
        <Row label="Turn chime" description="A soft two-tone chime when you stop talking and the model takes the turn.">
          <Switch checked={cue} onCheckedChange={useTalkSettings.getState().setCue} />
        </Row>
      </Section>
    </>
  )
}
