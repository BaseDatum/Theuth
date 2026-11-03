import type { ProviderInfo } from '@/rpc/bindings/ProviderInfo'
import type { RefreshedModels } from '@/rpc/bindings/RefreshedModels'

/** Display names of the built-in key providers (85 §0.9); the engine sends the ids. */
export const BUILTIN_LABEL: Record<string, string> = {
  opencode: 'OpenCode Zen',
  openrouter: 'OpenRouter',
}

/** The one Connections › Providers list (162 D2): built-ins first, then custom providers, engine order kept within each. */
export function orderProviders(list: ProviderInfo[] | undefined): ProviderInfo[] {
  return [...(list?.filter((p) => p.builtin) ?? []), ...(list?.filter((p) => !p.builtin) ?? [])]
}

/** Secret note written beside a built-in key, as the other Settings key forms do. */
export function builtinKeyNote(name: string) {
  return `${BUILTIN_LABEL[name] ?? name} key (Settings > Connections)`
}

/** Toast of a `refresh_provider_models` reply (162 D7): `rwx: 14 models (+2, −1)` or `rwx: 14 models, no change`. */
export function refreshedText({ provider: p, added, removed }: RefreshedModels): string {
  if (added.length === 0 && removed.length === 0) return `${p.name}: ${p.models} models, no change`
  return `${p.name}: ${p.models} models (+${added.length}, −${removed.length})`
}
