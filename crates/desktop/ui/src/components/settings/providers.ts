import type { ProviderInfo } from '@/rpc/extra'

/** Display names of the built-in key providers (85 §0.9); the engine sends the ids. */
export const BUILTIN_LABEL: Record<string, string> = {
  opencode: 'OpenCode Zen',
  openrouter: 'OpenRouter',
}

/** The `list_providers` reply split into the two Connections tables, order preserved. */
export function splitProviders(list: ProviderInfo[] | undefined) {
  const builtins = list?.filter((p) => p.builtin) ?? []
  const gateways = list?.filter((p) => !p.builtin) ?? []
  return { builtins, gateways }
}

/** Secret note written beside a built-in key, as the other Settings key forms do. */
export function builtinKeyNote(name: string) {
  return `${BUILTIN_LABEL[name] ?? name} key (Settings > Connections)`
}
