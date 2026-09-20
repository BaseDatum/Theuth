import { useState } from 'react'
import { Empty, Row, Section, refreshInfo, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { useApi } from '@/lib/rpc'
import { useApprovals } from '@/store/approvals'
import { useConnection } from '@/store/connection'

export function Approvals({ session, onClose }: { session: string | null; onClose: () => void }) {
  const api = useApi()
  const info = useConnection((s) => s.info)
  const pending = useApprovals((s) => s.pending.length)
  // No getter for one session's switch (`ApprovalsState.session` is only in
  // `set_approvals` replies); until the user flips it, show the default.
  const [sessionOn, setSessionOn] = useState<boolean | null>(null)

  const setGlobal = useAction((on: boolean) => api.set_approvals(null, on).then(() => refreshInfo(api)))
  const setSession = useAction((on: boolean) => api.set_approvals(session, on), {
    onSuccess: (state) => setSessionOn(state.session),
  })

  return (
    <>
      <Section title="Approvals">
        <Row label="Ask before risky tools" description="The default for every chat, saved to the config file.">
          <Switch
            checked={info?.approvals.global ?? false}
            disabled={!info || setGlobal.isPending}
            onCheckedChange={(v) => setGlobal.mutate(v)}
          />
        </Row>
        {session && (
          <Row label="Ask in this chat" description="Overrides the default for the open chat only.">
            <Switch
              checked={sessionOn ?? info?.approvals.global ?? false}
              disabled={!info || setSession.isPending}
              onCheckedChange={(v) => setSession.mutate(v)}
            />
          </Row>
        )}
      </Section>
      <Section title="Pending">
        {pending === 0 ? (
          <Empty>No tool is waiting for an answer.</Empty>
        ) : (
          <Row label={`${pending} waiting`} description="Across every chat.">
            <Button
              variant="outline"
              size="sm"
              onClick={() => {
                onClose()
                window.dispatchEvent(new CustomEvent('theuth:approvals'))
              }}
            >
              Review
            </Button>
          </Row>
        )}
      </Section>
    </>
  )
}
