import { useQueries, useQuery, useQueryClient } from '@tanstack/react-query'
import { type ReactNode, useEffect } from 'react'
import { Row, errorText, useAction } from '@/components/settings/shared'
import { Switch } from '@/components/ui/switch'
import { useRestartPending } from '@/lib/restart-pending'
import { useApi } from '@/lib/rpc'
import type { ConfigFlagKey } from '@/rpc/api'

const flagQuery = (key: ConfigFlagKey) => ['config-flag', key] as const

/** 188: one of the config flags that had no write path, as config.yaml has it and as the engine runs it. */
export function useConfigFlag(key: ConfigFlagKey) {
  const api = useApi()
  return useQuery({ queryKey: flagQuery(key), queryFn: () => api.config_flag_get(key), retry: false })
}

/**
 * 188 F1, F4–F6: the switch for such a flag, or for several keys that act as one (16 §5: Local network access is
 * `web.allow_private_network` and `browser.allow_private_network`): on only when every key is, and a change writes
 * them all. Its saved value shows at once; while the engine still runs another value the row says so and the change
 * is listed in Settings' restart banner under `name`, the feature's name.
 */
export function ConfigFlagRow({ flag, name, label, description }: { flag: ConfigFlagKey | readonly ConfigFlagKey[]; name: string; label: string; description: ReactNode }) {
  const api = useApi()
  const qc = useQueryClient()
  const keys: readonly ConfigFlagKey[] = typeof flag === 'string' ? [flag] : flag
  const states = useQueries({ queries: keys.map((key) => ({ queryKey: flagQuery(key), queryFn: () => api.config_flag_get(key), retry: false })) })
  // Several keys act as one: a write that fails part way puts the keys already written back.
  const save = useAction(async (on: boolean) => {
    const written: [ConfigFlagKey, boolean][] = []
    try {
      for (const [i, key] of keys.entries()) {
        qc.setQueryData(flagQuery(key), await api.config_flag_set(key, on))
        written.push([key, states[i]?.data?.enabled ?? !on])
      }
    } catch (e) {
      for (const [key, was] of written) await api.config_flag_set(key, was).catch(() => undefined)
      await Promise.all(keys.map((key) => qc.invalidateQueries({ queryKey: flagQuery(key) })))
      throw e
    }
  })
  const loaded = states.every((s) => s.data)
  const pending = states.some((s) => s.data?.restart_required)
  const failed = states.find((s) => s.isError)
  const id = keys.join('+')
  useEffect(() => {
    const restart = useRestartPending.getState()
    if (pending) restart.mark(id, name)
    else restart.clear(id)
  }, [pending, id, name])
  return (
    <Row
      label={label}
      description={
        <>
          {description}
          {pending && <span className="block">Applies after theuth restarts.</span>}
          {failed && <span className="block text-destructive">{errorText(failed.error)}</span>}
        </>
      }
    >
      <Switch
        aria-label={label}
        checked={loaded && states.every((s) => s.data?.enabled === true)}
        disabled={!loaded || save.isPending}
        onCheckedChange={(on) => save.mutate(on)}
      />
    </Row>
  )
}
