import { useQuery, useQueryClient } from '@tanstack/react-query'
import { type ReactNode, useEffect } from 'react'
import { Row, errorText, useAction } from '@/components/settings/shared'
import { Switch } from '@/components/ui/switch'
import { useRestartPending } from '@/lib/restart-pending'
import { useApi } from '@/lib/rpc'
import type { ConfigFlag, ConfigFlagKey } from '@/rpc/api'

const flagQuery = (key: ConfigFlagKey) => ['config-flag', key] as const

/** 188: one of the three `enabled` flags that had no write path, as config.yaml has it and as the engine runs it. */
export function useConfigFlag(key: ConfigFlagKey) {
  const api = useApi()
  return useQuery({ queryKey: flagQuery(key), queryFn: () => api.config_flag_get(key), retry: false })
}

/**
 * 188 F4–F6: the switch for such a flag. Its saved value shows at once; while the engine still runs the other value
 * the row says so and the change is listed in Settings' restart banner.
 */
export function ConfigFlagRow({ flag, label, description }: { flag: ConfigFlagKey; label: string; description: ReactNode }) {
  const api = useApi()
  const qc = useQueryClient()
  const state = useConfigFlag(flag)
  const save = useAction((on: boolean) => api.config_flag_set(flag, on), {
    onSuccess: (next: ConfigFlag) => qc.setQueryData(flagQuery(flag), next),
  })
  const pending = state.data?.restart_required ?? false
  useEffect(() => {
    const restart = useRestartPending.getState()
    if (pending) restart.mark(flag, label)
    else restart.clear(flag)
  }, [pending, flag, label])
  return (
    <Row
      label={label}
      description={
        <>
          {description}
          {pending && <span className="block">Applies after theuth restarts.</span>}
          {state.isError && <span className="block text-destructive">{errorText(state.error)}</span>}
        </>
      }
    >
      <Switch
        aria-label={label}
        checked={state.data?.enabled ?? false}
        disabled={!state.data || save.isPending}
        onCheckedChange={(on) => save.mutate(on)}
      />
    </Row>
  )
}
