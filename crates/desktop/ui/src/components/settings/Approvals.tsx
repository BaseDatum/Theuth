import { Row, Section, refreshInfo, useAction } from '@/components/settings/shared'
import { Switch } from '@/components/ui/switch'
import { useApi } from '@/lib/rpc'
import { useConnection } from '@/store/connection'

/** 188 A10: Settings › Agent › Approvals — the global default only; a chat's override and the waiting queue live with the chat. */
export function Approvals() {
  const api = useApi()
  const info = useConnection((s) => s.info)
  const setGlobal = useAction((on: boolean) => api.set_approvals(null, on).then(() => refreshInfo(api)))
  return (
    <Section title="Approvals">
      <Row label="Ask before risky tools" description="The default for every chat. Each chat can override it from its menu.">
        <Switch checked={info?.approvals.global ?? false} disabled={!info || setGlobal.isPending} onCheckedChange={(v) => setGlobal.mutate(v)} />
      </Row>
    </Section>
  )
}
