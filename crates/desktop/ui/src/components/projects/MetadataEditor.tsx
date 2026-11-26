import { Plus, X } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import type { Metadata } from './queries'

/** One editable key/value row; the editor keeps blanks and duplicates until `toMetadata` folds them. */
export type Pair = { key: string; value: string }

export const toPairs = (m: Metadata | null | undefined): Pair[] =>
  Object.entries(m ?? {})
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([key, value]) => ({ key, value }))

/** The rows as the wire's map: keys trimmed, blank keys dropped, a later duplicate wins. */
export const toMetadata = (pairs: readonly Pair[]): Metadata =>
  Object.fromEntries(pairs.flatMap((p) => (p.key.trim() ? [[p.key.trim(), p.value]] : [])))

/** Same map, compared key by key (order-free). */
export const sameMetadata = (a: Metadata, b: Metadata): boolean => {
  const ka = Object.keys(a)
  return ka.length === Object.keys(b).length && ka.every((k) => b[k] === a[k])
}

/** 223: a project's metadata as key/value rows (repo URL, default cwd, links …); Add row appends a blank one. */
export function MetadataEditor({ value, onChange, disabled }: { value: readonly Pair[]; onChange(next: Pair[]): void; disabled?: boolean }) {
  const set = (i: number, patch: Partial<Pair>) => onChange(value.map((p, j) => (j === i ? { ...p, ...patch } : p)))
  return (
    <div className="flex flex-col gap-1.5">
      {value.length === 0 && <p className="text-xs text-fg-tertiary">No metadata yet.</p>}
      {value.map((p, i) => (
        <div key={i} className="flex items-center gap-1.5">
          <Input
            dictate={false}
            className="h-8 w-40"
            placeholder="Key"
            aria-label={`Key ${i + 1}`}
            value={p.key}
            disabled={disabled}
            onChange={(e) => set(i, { key: e.target.value })}
          />
          <Input
            dictate={false}
            className="h-8 min-w-0 flex-1"
            placeholder="Value"
            aria-label={`Value ${i + 1}`}
            value={p.value}
            disabled={disabled}
            onChange={(e) => set(i, { value: e.target.value })}
          />
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            aria-label={`Remove ${p.key || `row ${i + 1}`}`}
            disabled={disabled}
            onClick={() => onChange(value.filter((_, j) => j !== i))}
          >
            <X />
          </Button>
        </div>
      ))}
      <Button type="button" variant="ghost" size="sm" className="self-start" disabled={disabled} onClick={() => onChange([...value, { key: '', value: '' }])}>
        <Plus /> Add field
      </Button>
    </div>
  )
}
