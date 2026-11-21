import { useQuerySupport } from '@/rpc/capabilities'
// Settings › Voice › Dictionary (153 §5.6, 204): the user and learned dictionary entries, with search over the built-in list
// (40k terms, searched on the engine through `dictionary_search`, never listed whole). Add / Edit / Override write a User
// entry through `dictionary_put`; Delete removes a User or Learned entry.

import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'
import { Empty, Section, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { cn } from '@/lib/utils'
import { useApi } from '@/lib/rpc'
import type { Entry, Scope, Source, Surface } from "@/rpc/bindings/method.talk.output"

/** Built-in matches shown at most (the engine counts the rest). */
export const BUILTIN_LIMIT = 100
/** The built-in list is searched only from this many characters on. */
export const BUILTIN_MIN_QUERY = 2
const EVERYWHERE = 'global'
const SURFACE_NAMES: [Surface, string][] = [
  ['composer', 'Composer'],
  ['tui', 'Terminal'],
  ['companion', 'Companion'],
  ['phone', 'Phone'],
  ['any_app', 'Other apps'],
]
const SOURCE_NAMES: [Source, string][] = [
  ['user', 'Yours'],
  ['learned', 'Learned'],
  ['packaged', 'Built-in'],
]
const keys = {
  list: ['voice', 'dictionary'] as const,
  counts: ['voice', 'dictionary-counts'] as const,
  search: (q: string) => ['voice', 'dictionary-search', q] as const,
}

/** Spoken forms then the variants' heard forms, de-duplicated case-insensitively. */
export function heardForms(e: Entry): string[] {
  const seen = new Set<string>()
  return [...e.spoken, ...e.variants.map((v) => v.heard)].filter((h) => {
    const k = h.toLowerCase()
    if (seen.has(k)) return false
    seen.add(k)
    return true
  })
}

/** Client-side filter of user + learned entries: case-insensitive substring of canonical, spoken or heard forms. */
export function filterEntries(entries: Entry[], query: string, sources: ReadonlySet<Source>): Entry[] {
  const q = query.trim().toLowerCase()
  return entries.filter((e) => sources.has(e.source) && (!q || [e.canonical, ...heardForms(e)].some((s) => s.toLowerCase().includes(q))))
}

export function scopeLabel(s: Scope): string {
  if (s.scope === 'global') return 'Everywhere'
  if (s.scope === 'surface') return SURFACE_NAMES.find(([id]) => id === s.surface)?.[1] ?? s.surface
  return s.app
}

/** "Heard as" field → spoken forms: comma-separated, trimmed, lower-cased, empties dropped. */
export function spokenForms(raw: string): string[] {
  return [...new Set(raw.split(',').map((s) => s.trim().toLowerCase()).filter(Boolean))]
}

type Form = { id: string; canonical: string; heard: string; scope: string; keepCase: boolean; base: Entry | null }

const blank: Form = { id: '', canonical: '', heard: '', scope: EVERYWHERE, keepCase: false, base: null }

/** The form for an entry: Edit keeps its id; Override (a built-in) starts a new User entry. */
export function formFor(e: Entry, override: boolean): Form {
  return {
    id: override ? '' : e.id,
    canonical: e.canonical,
    heard: e.spoken.join(', '),
    scope: e.scope.scope === 'surface' ? e.scope.surface : e.scope.scope === 'global' ? EVERYWHERE : EVERYWHERE,
    keepCase: e.tags.includes('keep_case'),
    base: override ? null : e,
  }
}

/** The `dictionary_put` argument: always a User entry; an empty id lets the engine mint one. */
export function entryFromForm(f: Form): Entry {
  const tags = (f.base?.tags ?? []).filter((t) => t !== 'keep_case')
  if (f.keepCase) tags.push('keep_case')
  const scope: Scope = f.scope === EVERYWHERE ? { scope: 'global' } : f.base?.scope.scope === 'app' ? f.base.scope : { scope: 'surface', surface: f.scope as Surface }
  return {
    id: f.id,
    canonical: f.canonical.trim(),
    kind: f.base?.kind ?? 'term',
    spoken: spokenForms(f.heard),
    variants: f.base?.variants ?? [],
    scope,
    source: 'user',
    tags,
  }
}

/** Voice › Dictionary. */
export function VoiceDictionary() {
  const canList = useQuerySupport(['dictionary_list'])
  const canCounts = useQuerySupport(['dictionary_counts'])
  const canSearch = useQuerySupport(['dictionary_search'])
  const api = useApi()
  const [query, setQuery] = useState('')
  const [sources, setSources] = useState<ReadonlySet<Source>>(new Set<Source>(['user', 'learned']))
  const [form, setForm] = useState<Form | null>(null)
  const list = useQuery({ enabled: canList, queryKey: keys.list, queryFn: () => api.dictionary_list(null) })
  const counts = useQuery({ enabled: canCounts, queryKey: keys.counts, queryFn: () => api.dictionary_counts() })
  const q = query.trim()
  const searchBuiltIn = canSearch && sources.has('packaged') && q.length >= BUILTIN_MIN_QUERY
  const builtIn = useQuery({ enabled: searchBuiltIn, queryKey: keys.search(q), queryFn: () => api.dictionary_search(q, 'packaged', BUILTIN_LIMIT) })
  const save = useAction(
    async (f: Form) => {
      const entry = entryFromForm(f)
      // `dictionary_put` upserts by canonical + scope: a renamed or re-scoped entry drops its old row first.
      if (f.base && (f.base.canonical !== entry.canonical || JSON.stringify(f.base.scope) !== JSON.stringify(entry.scope))) await api.dictionary_delete(f.base.id)
      return api.dictionary_put(entry)
    },
    { keys: [keys.list, keys.counts], onSuccess: () => setForm(null) },
  )
  const remove = useAction((id: string) => api.dictionary_delete(id), { keys: [keys.list, keys.counts] })

  const all = list.data ?? []
  const count = (s: Source) => (s === 'packaged' ? counts.data?.packaged : all.filter((e) => e.source === s).length)
  const rows = filterEntries(all, query, sources)
  const builtInRows = searchBuiltIn ? (builtIn.data?.entries ?? []) : []
  const toggle = (s: Source) =>
    setSources((prev) => {
      const next = new Set(prev)
      if (next.has(s)) next.delete(s)
      else next.add(s)
      return next
    })

  return (
    <Section title="Dictionary">
      <p className="pb-3 text-[13px] text-fg-secondary">
        Words and names dictation should write a particular way. Your entries and learned entries always apply; the built-in list only fixes capitals and
        joins names like GitHub.
      </p>
      <div className="flex flex-wrap items-center gap-2 pb-3">
        <Input dictate={false} aria-label="Search dictionary" placeholder="Search" className="w-56 max-md:w-full" value={query} onChange={(e) => setQuery(e.target.value)} />
        {SOURCE_NAMES.filter(([s]) => s !== 'packaged' || canSearch).map(([s, name]) => (
          <button
            key={s}
            type="button"
            aria-pressed={sources.has(s)}
            onClick={() => toggle(s)}
            className={cn('rounded-full border px-2.5 py-0.5 text-xs', sources.has(s) ? 'border-accent bg-accent/10 text-fg' : 'border-border text-fg-secondary')}
          >
            {name}
            {count(s) !== undefined && <span className="ml-1 tabular-nums text-fg-tertiary">{count(s)}</span>}
          </button>
        ))}
        <Button variant="outline" size="sm" className="ml-auto" onClick={() => setForm(blank)}>
          Add entry
        </Button>
      </div>
      {form && <EntryForm form={form} onChange={setForm} onCancel={() => setForm(null)} onSave={() => save.mutate(form)} busy={save.isPending} />}
      {list.isError && <Empty>Could not read the dictionary: {list.error.message}</Empty>}
      {list.isSuccess && all.length === 0 && <Empty>No entries yet. Add the names and terms dictation gets wrong, or correct a dictation and choose Add when asked.</Empty>}
      {rows.map((e) => (
        <EntryRow key={e.id} entry={e} onEdit={() => setForm(formFor(e, false))} onDelete={() => remove.mutate(e.id)} />
      ))}
      {sources.has('packaged') && canSearch && q.length < BUILTIN_MIN_QUERY && <Empty>Type at least {BUILTIN_MIN_QUERY} characters to search the built-in list.</Empty>}
      {builtInRows.map((e) => (
        <EntryRow key={e.id} entry={e} onOverride={() => setForm(formFor(e, true))} />
      ))}
      {searchBuiltIn && builtIn.data && builtIn.data.total > builtIn.data.entries.length && (
        <Empty>
          Showing {builtIn.data.entries.length} of {builtIn.data.total} — refine the search
        </Empty>
      )}
    </Section>
  )
}

function EntryRow({ entry, onEdit, onDelete, onOverride }: { entry: Entry; onEdit?: () => void; onDelete?: () => void; onOverride?: () => void }) {
  const heard = heardForms(entry)
  const source = SOURCE_NAMES.find(([s]) => s === entry.source)?.[1] ?? entry.source
  return (
    <div data-dictionary-row="" className="flex items-center gap-3 border-b border-border-subtle py-2.5 text-sm last:border-0 max-lg:flex-wrap">
      <span className={cn('min-w-0 font-semibold break-words', /[^\p{L} ]/u.test(entry.canonical) && 'font-mono')}>{entry.canonical}</span>
      <span className="flex min-w-0 flex-1 flex-wrap items-center gap-1">
        {heard.slice(0, 4).map((h) => (
          <span key={h} className="rounded-full border border-border px-2 py-0.5 text-xs text-fg-secondary">
            {h}
          </span>
        ))}
        {heard.length > 4 && <span className="text-xs text-fg-tertiary">+{heard.length - 4}</span>}
      </span>
      <span className="text-xs text-fg-tertiary">{scopeLabel(entry.scope)}</span>
      <span className="text-xs text-fg-tertiary">{source}</span>
      {onOverride && (
        <Button variant="ghost" size="sm" onClick={onOverride}>
          Override
        </Button>
      )}
      {onEdit && (
        <Button variant="ghost" size="sm" onClick={onEdit}>
          Edit
        </Button>
      )}
      {onDelete && (
        <Button variant="ghost" size="sm" onClick={onDelete}>
          Delete entry
        </Button>
      )}
    </div>
  )
}

function EntryForm({ form, onChange, onCancel, onSave, busy }: { form: Form; onChange: (f: Form) => void; onCancel: () => void; onSave: () => void; busy: boolean }) {
  const valid = form.canonical.trim() !== ''
  return (
    <form
      aria-label={form.base ? 'Edit entry' : 'Add entry'}
      className="mb-3 flex flex-col gap-3 rounded-lg border border-border p-3"
      onSubmit={(e) => {
        e.preventDefault()
        if (valid) onSave()
      }}
    >
      <label className="flex flex-col gap-1 text-sm">
        Written as
        <Input dictate={false} aria-label="Written as" required value={form.canonical} onChange={(e) => onChange({ ...form, canonical: e.target.value })} />
      </label>
      <label className="flex flex-col gap-1 text-sm">
        Heard as
        <Input dictate={false} aria-label="Heard as" placeholder="git hub, get hub" value={form.heard} onChange={(e) => onChange({ ...form, heard: e.target.value })} />
      </label>
      <div className="flex flex-wrap items-center gap-3 text-sm">
        Applies
        <Select value={form.scope} onValueChange={(v) => onChange({ ...form, scope: v })}>
          <SelectTrigger className="w-40" aria-label="Applies">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={EVERYWHERE}>Everywhere</SelectItem>
            {SURFACE_NAMES.map(([id, name]) => (
              <SelectItem key={id} value={id}>
                {name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <label className="flex items-center gap-2" title="The written form's casing stands: never capitalised at a sentence start, and a heard form differing only in case is left alone.">
          <input type="checkbox" aria-label="Keep exact casing" checked={form.keepCase} onChange={(e) => onChange({ ...form, keepCase: e.target.checked })} />
          Keep exact casing
        </label>
      </div>
      <div className="flex justify-end gap-2">
        <Button type="button" variant="ghost" size="sm" onClick={onCancel}>
          Cancel
        </Button>
        <Button type="submit" size="sm" disabled={!valid || busy}>
          Save
        </Button>
      </div>
    </form>
  )
}
