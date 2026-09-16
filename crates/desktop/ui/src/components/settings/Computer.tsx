import { useQuery } from '@tanstack/react-query'
import { openUrl } from '@tauri-apps/plugin-opener'
import { Check, X } from 'lucide-react'
import { toast } from 'sonner'
import { Empty, Row, Section, errorText } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { container } from '@/lib/platform'
import { queryKeys, useApi } from '@/lib/rpc'
import { LOCAL, useHarnesses } from '@/link/harnesses'

const PRIVACY = 'x-apple.systempreferences:com.apple.preference.security'

/** 27 §6: the two macOS permissions the `computer_*` tools need, plus the displays they will see. */
export function Computer() {
  const api = useApi()
  const perms = useQuery({
    queryKey: queryKeys.computer,
    queryFn: () => api.computer_permissions(),
  })
  const p = perms.data

  return (
    <Section
      title="Computer"
      actions={
        <Button
          variant="outline"
          size="sm"
          disabled={perms.isFetching}
          onClick={() => void perms.refetch()}
        >
          Refresh
        </Button>
      }
    >
      {perms.isError && <Empty>Computer control is disabled (`computer.enabled: false`)</Empty>}
      {p && (
        <>
          <Permission
            label="Accessibility"
            granted={p.accessibility}
            pane="Privacy_Accessibility"
          />
          <Permission
            label="Screen Recording"
            granted={p.screen_recording}
            pane="Privacy_ScreenCapture"
          />
          <Row
            label="Displays"
            description={p.displays.map((d) => (
              <div key={d.index}>
                {d.index} {d.name}
                {d.main ? ' main' : ''} {d.width}×{d.height} @{d.scale}×
              </div>
            ))}
          />
          {p.note && <p className="pt-2 text-[13px] text-fg-tertiary">{p.note}</p>}
        </>
      )}
    </Section>
  )
}

function Permission({ label, granted, pane }: { label: string; granted: boolean; pane: string }) {
  // The deep link opens *this* Mac's System Settings; over link the permissions are the harness's (31 §9).
  const local = useHarnesses((s) => s.active) === LOCAL && container() === 'macos'
  return (
    <Row
      label={
        <span className="flex items-center gap-2">
          {granted ? (
            <Check className="size-4 text-fg-secondary" />
          ) : (
            <X className="size-4 text-fg-secondary" />
          )}
          {label}
        </span>
      }
    >
      {local && (
        <Button
          variant="outline"
          size="sm"
          onClick={() => openUrl(`${PRIVACY}?${pane}`).catch((e) => toast.error(errorText(e)))}
        >
          Open System Settings
        </Button>
      )}
    </Row>
  )
}
