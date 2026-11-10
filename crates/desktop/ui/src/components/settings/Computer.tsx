import { useQuery } from '@tanstack/react-query'
import { openUrl } from '@tauri-apps/plugin-opener'
import { toast } from 'sonner'
import { ConfigFlagRow } from '@/components/settings/ConfigFlagRow'
import { useFlagRuns } from '@/components/settings/flag-runs'
import { Dot, Empty, Row, Section, errorText } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { container } from '@/lib/platform'
import { queryKeys, useApi } from '@/lib/rpc'
import { LOCAL, useEngines } from '@/link/engines'

const PRIVACY = 'x-apple.systempreferences:com.apple.preference.security'

/** 27 §6: the `computer_*` tools' switch (188 F6), then the two macOS permissions they need and the displays they will see. */
export function Computer() {
  const api = useApi()
  // Off, the engine has no computer actor to answer, so the permissions are not asked for.
  const runs = useFlagRuns('computer.enabled')
  const perms = useQuery({
    queryKey: queryKeys.computer,
    queryFn: () => api.computer_permissions(),
    enabled: runs === true,
  })
  const p = perms.data

  return (
    <Section
      title="Computer"
      actions={
        runs && (
          <Button
            variant="outline"
            size="sm"
            disabled={perms.isFetching}
            onClick={() => void perms.refetch()}
          >
            Refresh
          </Button>
        )
      }
    >
      <ConfigFlagRow flag="computer.enabled" name="Computer use" label="Enabled" description="Lets agents see and drive this desktop." />
      {runs === false && <Empty>The displays and permissions show once computer use runs.</Empty>}
      {runs && perms.isError && <Empty>{errorText(perms.error)}</Empty>}
      {runs && p && (
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
  // The deep link opens *this* Mac's System Settings; over link the permissions are the engine's (31 §9).
  const local = useEngines((s) => s.active) === LOCAL && container() === 'macos'
  return (
    <Row
      label={
        <span className="flex items-center gap-2">
          <Dot on={granted} />
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
