import { type QueryKey, type UseMutationResult, useMutation, useQueryClient } from '@tanstack/react-query'
import { isValidElement, type ReactNode, useState } from 'react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { chordOf } from '@/lib/chord'
import type { Api } from '@/rpc/bindings/client'
import type { SyncResult } from "@/rpc/bindings/method.agent.output"
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

/** One `label / description / control` line of a settings pane; the control sits right of the label. */
export function Row({
  label,
  description,
  stack,
  wide,
  children,
}: {
  label: ReactNode
  description?: ReactNode
  /** A control that is narrow on desktop (a model picker): right of the label there, dropped under it below `md`. */
  stack?: boolean
  /** A control that grows (chips, lists, switch or button groups, long text): always under the label at full width, at any width (169). */
  wide?: boolean
  children?: ReactNode
}) {
  const control = isValidElement<{ type?: string; role?: string; size?: string }>(children) ? children : null
  const nativeSmall = control?.type === 'input' && (control.props.type === 'checkbox' || control.props.type === 'radio')
  const small = nativeSmall || control?.type === Switch || control?.props.role === 'switch' || control?.props.role === 'checkbox' || control?.props.role === 'radio' || (control?.type === Button && control.props.size?.startsWith('icon'))
  const column = wide && !small
  const Control = nativeSmall ? 'label' : 'div'
  return (
    <div
      data-settings-row=""
      data-settings-small={small || undefined}
      className={
        column
          ? 'flex flex-col gap-3 border-b border-border-subtle py-4 last:border-0'
          : `flex items-center justify-between gap-6 border-b border-border-subtle py-4 last:border-0${small ? '' : ' max-lg:@max-[480px]/settings:flex-col max-lg:@max-[480px]/settings:items-stretch max-lg:@max-[480px]/settings:gap-3'}${stack && !small ? ' max-md:flex-col max-md:items-stretch max-md:gap-3' : ''}`
      }
    >
      <div className="min-w-0">
        <div className="text-sm">{label}</div>
        {description && <div className="mt-0.5 text-[13px] text-fg-secondary">{description}</div>}
      </div>
      {children && <Control data-settings-control="" className={`${column ? 'flex w-full items-center gap-2' : 'flex shrink-0 items-center gap-2'}${small ? '' : ' max-lg:@max-[480px]/settings:w-full max-lg:@max-[480px]/settings:flex-wrap'}`}>{children}</Control>}
    </div>
  )
}

/** A titled group of rows with optional header actions. */
export function Section({ title, actions, children }: { title: string; actions?: ReactNode; children: ReactNode }) {
  return (
    <section className="mb-8">
      <div data-settings-heading="" className="flex h-9 items-center justify-between">
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

/** Inline rename: Enter or blur saves a changed name, Escape keeps the old one. */
export function RenameInput({ value, label, onSave, onDone }: { value: string; label: string; onSave: (name: string) => void; onDone: () => void }) {
  return (
    <Input
      autoFocus
      defaultValue={value}
      aria-label={label}
      className="h-7 w-48"
      onBlur={(e) => {
        const v = e.currentTarget.value.trim()
        if (v && v !== value) onSave(v)
        onDone()
      }}
      onKeyDown={(e) => {
        if (e.key === 'Escape') e.currentTarget.value = value
        if (e.key === 'Enter' || e.key === 'Escape') e.currentTarget.blur()
      }}
    />
  )
}

/**
 * A whole-number `Info` field edited on blur/Enter (Agent loop, Advisor); values outside `[min, max]` are ignored.
 * `placeholder`: a key the engine may not send (an optional one); the field stays editable and shows the engine default.
 * `step` below 1: a decimal field (USD amounts), not floored.
 * `blank`: the value an emptied field writes (201: "empty = automatic" writes 0); without it an empty field writes nothing.
 */
export function Count({
  value,
  min,
  max,
  step,
  placeholder,
  blank,
  action,
  'aria-label': ariaLabel,
}: {
  value: number | undefined
  min: number
  max?: number
  step?: number
  placeholder?: number | string
  blank?: number
  action: { mutate: (n: number) => void; isPending: boolean }
  'aria-label'?: string
}) {
  return (
    <Input
      key={value}
      type="number"
      aria-label={ariaLabel}
      min={min}
      max={max}
      step={step}
      defaultValue={value}
      placeholder={placeholder?.toString()}
      disabled={(value === undefined && placeholder === undefined) || action.isPending}
      className="w-28 text-right"
      onBlur={(e) => {
        const raw = e.currentTarget.value
        const n = blank !== undefined && raw.trim() === '' ? blank : bounded(raw, min, max, step)
        if (n !== undefined && n !== (value ?? blank)) action.mutate(n)
      }}
      onKeyDown={(e) => {
        if (e.key === 'Enter') e.currentTarget.blur()
      }}
    />
  )
}

/** A native range: `onChange` follows the pointer (the label beside it previews), `onCommit` fires on release so a drag is one write. */
export function Range({ value, min, max, step, onChange, onCommit, disabled, 'aria-label': ariaLabel }: { value: number; min: number; max: number; step: number; onChange(v: number): void; onCommit(v: number): void; disabled: boolean; 'aria-label': string }) {
  return (
    <input
      type="range"
      aria-label={ariaLabel}
      className="w-48 accent-primary disabled:opacity-50 max-md:w-full"
      min={min}
      max={max}
      step={step}
      value={value}
      disabled={disabled}
      onChange={(e) => onChange(Number(e.target.value))}
      onPointerUp={(e) => onCommit(Number(e.currentTarget.value))}
      onKeyUp={(e) => onCommit(Number(e.currentTarget.value))}
    />
  )
}

/** The number a count field accepts from `raw`: floored (unless `step` < 1), within `[min, max]`; `undefined` otherwise, an empty field included (the field is left alone). */
export function bounded(raw: string | number, min: number, max = Number.POSITIVE_INFINITY, step = 1): number | undefined {
  if (String(raw).trim() === '') return undefined
  const n = step < 1 ? Number(raw) : Math.floor(Number(raw))
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
 * `toast.error` on failure (or `onError` instead, when given). `V` is whatever the caller's closure needs.
 */
export function useAction<V, R>(
  fn: (vars: V) => Promise<R>,
  opts: { keys?: readonly QueryKey[]; onSuccess?: (result: R, vars: V) => void; onError?: (e: unknown) => void } = {},
): Action<V, R> {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: fn,
    onSuccess: (result, vars) => {
      for (const key of opts.keys ?? []) void qc.invalidateQueries({ queryKey: key })
      opts.onSuccess?.(result, vars)
    },
    onError: (e) => (opts.onError ? opts.onError(e) : toast.error(errorText(e))),
  })
}

/** Re-read `info()` into the connection store after a setting it carries changed. */
export async function refreshInfo(api: Api): Promise<void> {
  useConnection.getState().setInfo(await api.info())
}
