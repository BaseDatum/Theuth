// 227: the copyable fields' shapes, so a tab states a field in one line: which setter puts a value, how it reads back.

import type { CopyField } from '@/components/settings/copy/set-on-all'
import type { Api } from '@/rpc/bindings/client'
import type { Info } from '@/rpc/bindings/method.rpc_2dcompose.output'

type Read<T> = CopyField<T>['read']

/** Read back from another machine's `info()`. */
export const fromInfo = <T>(pick: (info: Info) => T): Read<T> => ({ key: 'info', pick: (raw) => pick(raw as Info) })

/** Read back from another machine's no-argument getter `key` (its answer, or `pick` of it). */
export const fromGetter = <T>(key: keyof Api, pick: (raw: unknown) => T = (raw) => raw as T): Read<T> => ({ key, pick })

/** A field written inline: infers `T` from `value`, so `format`/`plan` see the value's type. */
export const copyField = <T>(field: CopyField<T>): CopyField => field as CopyField

/**
 * A field whose value one setter takes: `method(...args(value))` (default `method(value)`) on every machine.
 * `undefined` while the tab has not loaded the value: no copy controls yet.
 */
export function setterField<T>(
  id: string,
  label: string,
  value: T | undefined,
  method: keyof Api,
  opts: { args?: (value: T) => unknown[]; read?: Read<T>; format?: (value: T) => string; hubOnly?: boolean } = {},
): CopyField<T> | undefined {
  if (value === undefined) return undefined
  const args = opts.args ?? ((v: T) => [v])
  return {
    id,
    label,
    value,
    format: opts.format ?? formatValue,
    plan: (v) => ({ kind: 'calls', calls: [{ method, args: args(v) }] }),
    read: opts.read,
    hubOnly: opts.hubOnly,
  }
}

/** A plain value as the sheet shows it: on/off, numbers, text, `none` for null. */
export function formatValue(value: unknown): string {
  if (value === null || value === undefined) return 'none'
  if (typeof value === 'boolean') return value ? 'on' : 'off'
  if (typeof value === 'string' || typeof value === 'number') return String(value)
  return JSON.stringify(value)
}
