import { type QueryKey, type UseMutationResult, useMutation, useQueryClient } from '@tanstack/react-query'
import type { ReactNode } from 'react'
import { toast } from 'sonner'
import type { Api } from '@/rpc/api'
import type { SyncResult } from '@/rpc/bindings/SyncResult'
import { useConnection } from '@/store/connection'

/** One `label / description / control` line of a settings pane. */
export function Row({ label, description, children }: { label: ReactNode; description?: ReactNode; children?: ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-6 border-b border-border-subtle py-4 last:border-0">
      <div className="min-w-0">
        <div className="text-sm">{label}</div>
        {description && <div className="mt-0.5 text-[13px] text-fg-secondary">{description}</div>}
      </div>
      {children && <div className="flex shrink-0 items-center gap-2">{children}</div>}
    </div>
  )
}

/** A titled group of rows with optional header actions. */
export function Section({ title, actions, children }: { title: string; actions?: ReactNode; children: ReactNode }) {
  return (
    <section className="mb-8">
      <div className="flex h-9 items-center justify-between">
        <h3 className="text-base font-semibold">{title}</h3>
        {actions && <div className="flex items-center gap-2">{actions}</div>}
      </div>
      {children}
    </section>
  )
}

/** Dimmed line for an empty list. */
export function Empty({ children }: { children: ReactNode }) {
  return <p className="py-4 text-sm text-fg-tertiary">{children}</p>
}

/** The message of a thrown value, for toasts. */
export function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

/** Show `results` of `sync_mcp`/`sync_skills` as one toast. */
export function toastSync(results: SyncResult[]): void {
  if (results.length === 0) toast.message('Nothing to sync')
  else toast.message('Synced', { description: results.map((r) => `${r.name}: ${r.status}`).join('\n') })
}

/** What `useAction` hands back: call `.mutate(vars)`, read `.isPending`. */
export type Action<V, R = unknown> = UseMutationResult<R, Error, V>

/**
 * `useMutation` with the settings conventions: invalidate `keys` on success,
 * `toast.error` on failure. `V` is whatever the caller's closure needs.
 */
export function useAction<V, R>(
  fn: (vars: V) => Promise<R>,
  opts: { keys?: readonly QueryKey[]; onSuccess?: (result: R, vars: V) => void } = {},
): Action<V, R> {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: fn,
    onSuccess: (result, vars) => {
      for (const key of opts.keys ?? []) void qc.invalidateQueries({ queryKey: key })
      opts.onSuccess?.(result, vars)
    },
    onError: (e) => toast.error(errorText(e)),
  })
}

/** Re-read `info()` into the connection store after a setting it carries changed. */
export async function refreshInfo(api: Api): Promise<void> {
  useConnection.getState().setInfo(await api.info())
}
