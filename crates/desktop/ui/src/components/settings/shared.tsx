import { type QueryKey, type UseMutationResult, useMutation, useQueryClient } from '@tanstack/react-query'
import { type ReactNode, useState } from 'react'
import { toast } from 'sonner'
import { Input } from '@/components/ui/input'
import { chordOf } from '@/lib/chord'
import type { Api } from '@/rpc/api'
import type { SyncResult } from '@/rpc/bindings/SyncResult'
import { useConnection } from '@/store/connection'

/** The one connected / logged-in / online indicator of every settings pane: green on, faint grey off. */
export function Dot({ on }: { on: boolean }) {
  return <span className={`inline-block size-2 shrink-0 rounded-full ${on ? 'bg-success' : 'bg-fg-tertiary/40'}`} />
}

/**
 * A chord reader: focus, press the chord, done. Backspace restores `fallback`; a bare Escape cancels (so bind
 * Escape itself only as a default). `commit` may reject (a global shortcut another app holds, a chord already
 * bound): the message toasts and the old chord stays. `bare` accepts a key with no modifier (the composer's editor keys).
 */
export function ChordInput({
  value,
  fallback,
  bare,
  display,
  commit,
}: {
  value: string
  fallback: string
  bare?: boolean
  display?: (chord: string) => string
  commit: (chord: string) => Promise<void> | void
}) {
  const [reading, setReading] = useState(false)
  return (
    <Input
      readOnly
      className="w-40 text-center font-mono"
      value={reading ? 'press a chord…' : (display?.(value) ?? value)}
      onFocus={() => setReading(true)}
      onBlur={() => setReading(false)}
      onKeyDown={(e) => {
        e.preventDefault()
        const el = e.currentTarget
        if (e.key === 'Escape' && !e.shiftKey && !e.altKey && !e.ctrlKey && !e.metaKey) return el.blur()
        if (e.key === 'Backspace') {
          void Promise.resolve(commit(fallback)).finally(() => el.blur())
          return
        }
        const chord = chordOf(e.nativeEvent, bare)
        if (!chord) return
        if (chord === value) return el.blur()
        void Promise.resolve(commit(chord))
          .catch((err: unknown) => toast.error(err instanceof Error ? err.message : `Could not bind ${chord}; another app holds it`))
          .finally(() => el.blur())
      }}
    />
  )
}

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

/** A whole-number `Info` field edited on blur/Enter (Agent loop, Advisor); values outside `[min, max]` are ignored. */
export function Count({
  value,
  min,
  max,
  action,
}: {
  value: number | undefined
  min: number
  max?: number
  action: { mutate: (n: number) => void; isPending: boolean }
}) {
  return (
    <Input
      key={value}
      type="number"
      min={min}
      max={max}
      defaultValue={value}
      disabled={value === undefined || action.isPending}
      className="w-28 text-right"
      onBlur={(e) => {
        const n = bounded(e.currentTarget.value, min, max)
        if (n !== undefined && n !== value) action.mutate(n)
      }}
      onKeyDown={(e) => {
        if (e.key === 'Enter') e.currentTarget.blur()
      }}
    />
  )
}

/** The whole number a count field accepts from `raw`: floored, within `[min, max]`; `undefined` otherwise (the field is left alone). */
export function bounded(raw: string | number, min: number, max = Number.POSITIVE_INFINITY): number | undefined {
  const n = Math.floor(Number(raw))
  return Number.isFinite(n) && n >= min && n <= max ? n : undefined
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
