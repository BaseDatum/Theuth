import { Row, Section } from '@/components/settings/shared'
import { Switch } from '@/components/ui/switch'
import { useToolRuns } from '@/lib/tool-runs'

/** Settings › Chat: how the transcript displays. Client-only preferences; nothing here changes what the engine sends. */
export function Chat() {
  const collapse = useToolRuns((s) => s.collapse)
  return (
    <Section title="Tool calls">
      <Row
        label="Collapse tool calls by default"
        description="Consecutive tool calls in a reply fold into one card with a live status line; click it to see every call. Off shows each call as its own card."
      >
        <Switch checked={collapse} onCheckedChange={useToolRuns.getState().setCollapse} />
      </Row>
    </Section>
  )
}
