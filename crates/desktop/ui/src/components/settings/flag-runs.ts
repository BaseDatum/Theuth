import { useConfigFlag } from '@/components/settings/ConfigFlagRow'
import type { ConfigFlag } from "@/rpc/bindings/method.rpc_2dcompose.output"
import type { ConfigFlagKey } from '@/rpc/bindings/method.rpc_2dcompose.input'

/**
 * 188 F4/F6: whether the engine runs with a flag on, so a tab knows whether to show (and query) what needs it; `null`
 * until known. An engine that cannot answer `config_flag_get` counts as running: the tab shows what it can, as before.
 */
export function flagRuns(flag: { data?: ConfigFlag; isError: boolean }): boolean | null {
  if (flag.data) return flag.data.running
  return flag.isError ? true : null
}

export const useFlagRuns = (key: ConfigFlagKey): boolean | null => flagRuns(useConfigFlag(key))
