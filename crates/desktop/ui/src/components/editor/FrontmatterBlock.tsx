import { createReactBlockSpec } from '@blocknote/react'
import { frontmatterConfig } from '@theuth/documents/schema'
import { type Entry, emitHeader, parseHeader } from '@theuth/documents/yaml'
import { Check, Pencil, Plus, X } from 'lucide-react'
import { type KeyboardEvent, type ReactNode, useRef, useState } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { DictationButton } from '@/components/ui/dictation'
import { Input } from '@/components/ui/input'
import { cn } from 'cn'

/** The five types the guidance names (10 §2.2); types are open, so the current value is offered too. */
const KNOWN_TYPES = ['fact', 'entity', 'decision', 'procedure', 'project']
/** Fixed rows, in header order; everything else is a free row. */
const FIXED = ['type', 'title', 'description', 'tags', 'scope', 'source', 'timestamp']
/** The store sets these (10 §2.2 `source`, `timestamp`); shown, not edited. */
const READ_ONLY: Record<string, true> = { source: true, timestamp: true }

type Props = {
  yaml: string
  onChange: (yaml: string) => void
}

/**
 * The header as the pre-59 concept header (59 §24.y): title, type badge, tag chips, description, the
 * scope/source/timestamp line. A pencil (hover; always on fingers) or a click opens the form of {@link FrontmatterForm}.
 */
export function Frontmatter({ yaml, onChange }: Props) {
  const [editing, setEditing] = useState(false)
  if (editing) return <FrontmatterForm yaml={yaml} onChange={onChange} onDone={() => setEditing(false)} />
  const entries = parseHeader(yaml)
  const value = (key: string) => {
    const e = entries.find((x) => x.key === key)
    return !e || 'raw' in e ? '' : 'list' in e ? e.list.join(', ') : e.value
  }
  const tagsEntry = entries.find((e) => e.key === 'tags')
  const tags = tagsEntry && 'list' in tagsEntry ? tagsEntry.list : value('tags') === '' ? [] : value('tags').split(/,\s*/)
  const title = value('title')
  const type = value('type')
  const facts = ['scope', 'source', 'timestamp'].map(value).filter((s) => s !== '')
  return (
    <header
      className="group relative mb-6 w-full cursor-default border-b border-border-subtle pb-4"
      contentEditable={false}
      data-frontmatter
      onDoubleClick={() => setEditing(true)}
    >
      <div className="flex flex-wrap items-center gap-2 pr-8">
        <h1 className={cn('text-[24px] leading-8 font-semibold', title === '' && 'text-fg-tertiary')}>{title || 'Untitled'}</h1>
        <Badge variant="outline" className={cn(type === '' && 'border-destructive text-danger')}>
          {type || 'type?'}
        </Badge>
        {tags.map((t) => (
          <Badge key={t} variant="secondary">
            {t}
          </Badge>
        ))}
      </div>
      {value('description') !== '' && <p className="mt-1 text-fg-secondary">{value('description')}</p>}
      {facts.length > 0 && <p className="mt-1 text-sm text-fg-tertiary">{facts.join(' · ')}</p>}
      <Button
        variant="ghost"
        size="icon-sm"
        aria-label="Edit header"
        className="absolute top-0 right-0 opacity-0 group-hover:opacity-100 focus-visible:opacity-100 pointer-coarse:opacity-100"
        onClick={() => setEditing(true)}
      >
        <Pencil />
      </Button>
    </header>
  )
}

/**
 * The header as a two-column form (59 §4.5): `type` a select, `tags` chips, `source`/`timestamp` read-only,
 * unknown keys as free rows, "+ add key". The only validation `knowledge::Put` has is a non-empty `type`;
 * that row turns red. Invalid YAML the subset cannot model (`raw` entries) is kept verbatim.
 */
function FrontmatterForm({ yaml, onChange, onDone }: Props & { onDone: () => void }) {
  const entries = parseHeader(yaml)
  const update = (next: Entry[]) => onChange(emitHeader(next).replace(/\n$/, ''))
  const value = (key: string) => {
    const e = entries.find((x) => x.key === key)
    return !e || 'raw' in e ? '' : 'list' in e ? e.list.join(', ') : e.value
  }
  const put = (key: string, v: string | string[]) => {
    const at = entries.findIndex((e) => e.key === key)
    const entry: Entry = Array.isArray(v) ? { key, list: v } : { key, value: v }
    update(at === -1 ? [...entries, entry] : entries.map((e, i) => (i === at ? entry : e)))
  }
  const type = value('type')
  const tagsEntry = entries.find((e) => e.key === 'tags')
  const tags = tagsEntry && 'list' in tagsEntry ? tagsEntry.list : value('tags') === '' ? [] : value('tags').split(/,\s*/)
  const free = entries.filter((e) => !FIXED.includes(e.key))
  // "+ add key" stays local until it has a name: an empty key is not valid YAML and must never reach the file.
  const [draft, setDraft] = useState<{ key: string; value: string } | null>(null)
  const commitDraft = () => {
    if (draft && draft.key.trim() !== '') put(draft.key.trim(), draft.value)
    setDraft(null)
  }

  return (
    <div
      className="my-2 grid w-full grid-cols-[minmax(80px,auto)_1fr] gap-x-3 gap-y-1 rounded-md border border-border-subtle bg-layer-1/40 p-3 text-sm"
      contentEditable={false}
      data-frontmatter
    >
      <Row label="type" invalid={type === ''}>
        <select
          aria-invalid={type === ''}
          className={cn(
            'h-8 w-full rounded-md border border-input bg-transparent px-2 text-sm outline-none focus-visible:border-ring',
            type === '' && 'border-destructive',
          )}
          value={type}
          onChange={(e) => put('type', e.target.value)}
        >
          {type === '' && <option value="">(required)</option>}
          {(KNOWN_TYPES.includes(type) || type === '' ? KNOWN_TYPES : [type, ...KNOWN_TYPES]).map((t) => (
            <option key={t} value={t}>
              {t}
            </option>
          ))}
        </select>
      </Row>
      <Row label="title">
        <Text value={value('title')} onChange={(v) => put('title', v)} />
      </Row>
      <Row label="description">
        <Text value={value('description')} onChange={(v) => put('description', v)} />
      </Row>
      <Row label="tags">
        <Tags tags={tags} onChange={(v) => put('tags', v)} />
      </Row>
      <Row label="scope">
        <Text value={value('scope')} onChange={(v) => put('scope', v)} placeholder="absolute directory, or empty" />
      </Row>
      {['source', 'timestamp'].map(
        (k) =>
          value(k) !== '' && (
            <Row key={k} label={k}>
              <span className="truncate font-mono text-xs leading-8 text-fg-tertiary" title={READ_ONLY[k] && 'set by the store'}>
                {value(k)}
              </span>
            </Row>
          ),
      )}
      {free.map((e, i) =>
        'raw' in e ? (
          <Row key={e.key} label={e.key}>
            <pre className="whitespace-pre-wrap font-mono text-xs leading-8 text-fg-tertiary">{e.raw.join('\n')}</pre>
          </Row>
        ) : (
          <FreeRow
            key={`${i}:${e.key}`}
            entry={e}
            onChange={(next) => update(entries.map((x) => (x === e ? next : x)))}
            onRemove={() => update(entries.filter((x) => x !== e))}
          />
        ),
      )}
      {draft && (
        <div
          className="col-span-2 grid grid-cols-subgrid"
          onBlur={(e) => !e.currentTarget.contains(e.relatedTarget) && commitDraft()}
          onKeyDown={(e) => e.key === 'Enter' && commitDraft()}
        >
          <Input
            className="h-8 font-mono text-base shadow-none md:text-xs"
            value={draft.key}
            placeholder="key"
            autoFocus
            onChange={(e) => setDraft({ ...draft, key: e.target.value })}
          />
          <Input
            className="h-8 shadow-none md:text-sm"
            value={draft.value}
            placeholder="value"
            onChange={(e) => setDraft({ ...draft, value: e.target.value })}
          />
        </div>
      )}
      {/* 59 §10: fields editable on a phone, no new keys (the desktop adds them). */}
      <div className="col-span-2 flex items-center">
        <Button
          variant="ghost"
          size="sm"
          className="h-7 px-2 text-xs text-fg-tertiary pointer-coarse:hidden"
          onClick={() => setDraft({ key: '', value: '' })}
        >
          <Plus className="size-3" /> add key
        </Button>
        <Button variant="ghost" size="sm" className="ml-auto h-7 px-2 text-xs" onClick={onDone}>
          <Check className="size-3" /> Done
        </Button>
      </div>
    </div>
  )
}

function Row({ label, invalid, children }: { label: string; invalid?: boolean; children: ReactNode }) {
  return (
    <>
      <span className={cn('truncate font-mono text-xs leading-8 text-fg-secondary', invalid && 'text-danger')}>{label}</span>
      <div className="min-w-0">{children}</div>
    </>
  )
}

function Text({ value, onChange, placeholder }: { value: string; onChange: (v: string) => void; placeholder?: string }) {
  return (
    <Input
      className="h-8 shadow-none md:text-sm"
      value={value}
      placeholder={placeholder}
      onChange={(e) => onChange(e.target.value)}
    />
  )
}

/** Unknown keys: both cells editable; a blank key drops the row on blur. */
function FreeRow({
  entry,
  onChange,
  onRemove,
}: {
  entry: { key: string; value: string } | { key: string; list: string[] }
  onChange: (next: Entry) => void
  onRemove: () => void
}) {
  const [key, setKey] = useState(entry.key)
  const text = 'list' in entry ? entry.list.join(', ') : entry.value
  return (
    <>
      <Input
        className="h-8 font-mono text-base shadow-none md:text-xs"
        value={key}
        placeholder="key"
        autoFocus={entry.key === ''}
        onChange={(e) => setKey(e.target.value)}
        onBlur={() => {
          const k = key.trim()
          if (k === '') onRemove()
          else if (k !== entry.key) onChange('list' in entry ? { key: k, list: entry.list } : { key: k, value: entry.value })
        }}
      />
      <div className="flex min-w-0 items-center gap-1">
        <Input
          className="h-8 shadow-none md:text-sm"
          value={text}
          onChange={(e) => onChange({ key: entry.key, value: e.target.value })}
        />
        <Button variant="ghost" size="icon-sm" aria-label={`Remove ${entry.key}`} onClick={onRemove}>
          <X className="size-3" />
        </Button>
      </div>
    </>
  )
}

/** Chips; Enter or comma adds, Backspace on an empty input removes the last. */
function Tags({ tags, onChange }: { tags: string[]; onChange: (tags: string[]) => void }) {
  const [draft, setDraft] = useState('')
  const input = useRef<HTMLInputElement>(null)
  const commit = () => {
    const t = draft.trim()
    if (t !== '' && !tags.includes(t)) onChange([...tags, t])
    setDraft('')
  }
  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter' || e.key === ',') {
      e.preventDefault()
      commit()
    } else if (e.key === 'Backspace' && draft === '' && tags.length > 0) {
      onChange(tags.slice(0, -1))
    }
  }
  return (
    <div className="flex min-h-8 flex-wrap items-center gap-1 rounded-md border border-input px-2 py-1">
      {tags.map((t) => (
        <span key={t} className="inline-flex items-center gap-0.5 rounded-sm bg-layer-2 px-1.5 text-xs">
          {t}
          <button type="button" aria-label={`Remove ${t}`} className="text-fg-tertiary hover:text-fg-primary" onClick={() => onChange(tags.filter((x) => x !== t))}>
            <X className="size-3" />
          </button>
        </span>
      ))}
      <input
        ref={input}
        className="min-w-16 flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground"
        value={draft}
        placeholder={tags.length === 0 ? 'tag, tag' : ''}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={onKeyDown}
        onBlur={commit}
      />
      <DictationButton field={input} />
    </div>
  )
}

/** The block spec the editor's schema uses: shared config, this render, not selectable/draggable (59 §4.5). */
export const frontmatterBlock = createReactBlockSpec(frontmatterConfig, {
  meta: { selectable: false, isolating: true },
  render: ({ block, editor }) => (
    <Frontmatter
      yaml={block.props.yaml}
      onChange={(yaml) => editor.updateBlock(block, { props: { yaml } })}
    />
  ),
})()
