// 216 §9.4, contract §12.4 / §14.1: the OpenUI tool bridge of a plugin surface. The renderer reports the statement
// it runs (`ProviderContext.statement_id`); the host classified every statement when it validated the source
// (`SurfaceContent.bindings`), so a Mutation goes to `plugin_action` (only with the press's action id) and a Query to
// `plugin_query` — whether it loads or is pressed. An unknown statement never reaches the plugin.
import type { AppRendererProps } from '@theuth/apps/renderer'
import type { Api } from '@/rpc/bindings/client'
import type { SurfaceBinding } from '@/rpc/bindings/method.plugins.output'

export function pluginProvider(
  api: Pick<Api, 'plugin_action' | 'plugin_query'>,
  p: { plugin: string; placement: string; session: string | null; bindings: readonly SurfaceBinding[] },
): AppRendererProps['provider'] {
  const byId = new Map(p.bindings.map((b) => [b.statement_id, b]))
  return (_tool, args, ctx) => {
    const b = byId.get(ctx.statement_id)
    if (!b) return Promise.reject(new Error(`unknown statement ${ctx.statement_id}`))
    if (b.kind === 'mutation') {
      if (!ctx.action_id) return Promise.reject(new Error('a mutation needs a press'))
      // `form` is null: the renderer hands the provider only the statement's evaluated arguments, so OpenUI form
      // fields reach the plugin folded into `args` (bound through `$state`), never as a separate form object.
      return api.plugin_action(p.plugin, p.placement, b.name, args ?? {}, p.session, ctx.action_id, null)
    }
    return api.plugin_query(p.plugin, p.placement, b.name, args ?? {}, p.session)
  }
}
