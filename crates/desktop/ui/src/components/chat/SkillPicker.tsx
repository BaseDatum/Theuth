import { Check, ChevronLeft, ChevronRight, MessageSquarePlus, Sparkles, X } from 'lucide-react'
import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import type { SuffixScope, Suffix } from '@/lib/suffixes'
import type { SkillMeta } from '@/rpc/bindings/method.agent.output'
import { useQuerySupport } from '@/rpc/capabilities'

/** Skills whose name or description holds every word of `query` (any case), in the index's order. */
export function filterSkills(skills: readonly SkillMeta[], query: string): SkillMeta[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean)
  return skills.filter((s) => {
    const hay = `${s.name} ${s.description}`.toLowerCase()
    return words.every((w) => hay.includes(w))
  })
}

/** The `+` menu's Skills entry; hidden when the engine can neither list nor use skills. */
export function SkillsEntry({ onOpen }: { onOpen: () => void }) {
  const supported = useQuerySupport(['list_skills', 'use_skill'])
  if (!supported) return null
  return (
    <Button variant="ghost" className="w-full justify-start" onClick={onOpen}>
      <Sparkles /> Skills
      <ChevronRight className="ml-auto" />
    </Button>
  )
}

/** The `+` menu's skill step: a searchable `list_skills` index; a pick parks the skill (`parked` ones are ticked). */
export function SkillPicker({
  skills,
  parked,
  onPick,
  onBack,
}: {
  /** `undefined` while the index loads. */
  skills: readonly SkillMeta[] | undefined
  parked: readonly string[]
  onPick: (name: string) => void
  onBack: () => void
}) {
  const [query, setQuery] = useState('')
  const shown = filterSkills(skills ?? [], query)
  return (
    <div role="menu" aria-label="Skills">
      <Button variant="ghost" className="w-full justify-start text-fg-secondary" onClick={onBack}>
        <ChevronLeft /> Skills
      </Button>
      <Input
        autoFocus
        value={query}
        placeholder="Search skills"
        aria-label="Search skills"
        className="my-1 h-8"
        onChange={(e) => setQuery(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && shown[0]) {
            e.preventDefault()
            onPick(shown[0].name)
          }
        }}
      />
      <div className="max-h-72 overflow-y-auto">
        {skills === undefined ? (
          <p className="px-3 py-1.5 text-sm text-fg-tertiary">Loading skills…</p>
        ) : shown.length === 0 ? (
          <p className="px-3 py-1.5 text-sm text-fg-tertiary">{skills.length ? 'No skill matches.' : 'No skills yet.'}</p>
        ) : (
          shown.map((s) => (
            <Button
              key={s.name}
              variant="ghost"
              role="menuitemcheckbox"
              aria-checked={parked.includes(s.name)}
              data-skill={s.name}
              className="h-auto w-full flex-col items-start gap-0.5 py-1.5 text-left whitespace-normal"
              onClick={() => onPick(s.name)}
            >
              <span className="flex w-full items-center gap-1.5 font-medium">
                <span className="truncate">{s.name}</span>
                {parked.includes(s.name) && <Check className="ml-auto size-3.5 shrink-0" />}
              </span>
              {s.description && <span className="line-clamp-2 text-xs font-normal text-fg-tertiary">{s.description}</span>}
            </Button>
          ))
        )}
      </div>
    </div>
  )
}

/** One chip per parked skill, in send order, each with its own ✕. 215 C2: bare chips, the caller's `PillRow` lays them out. */
export function SkillChips({ names, onRemove }: { names: readonly string[]; onRemove: (name: string) => void }) {
  return (
    <>
      {names.map((name) => (
        <span
          key={name}
          data-skill-chip={name}
          className="flex min-w-0 max-w-[240px] items-center gap-1.5 rounded-full bg-layer-2 px-3 py-1 text-sm text-fg-secondary"
        >
          <Sparkles className="size-3.5 shrink-0" />
          <span className="truncate font-medium">{name}</span>
          <button type="button" aria-label={`Remove skill ${name}`} onClick={() => onRemove(name)} className="ml-1 shrink-0 hover:text-foreground">
            <X className="size-3" />
          </button>
        </span>
      ))}
    </>
  )
}

/**
 * 215 C2/G5: the pills above the input box — parked skills, then the active prompt suffix — in one left-aligned row
 * that wraps only when out of room; nothing when there is no pill.
 */
export function PillRow({
  skills,
  onRemoveSkill,
  suffix,
  onClearSuffix,
}: {
  skills: readonly string[]
  onRemoveSkill: (name: string) => void
  suffix: (Suffix & { scope: SuffixScope }) | null
  onClearSuffix: () => void
}) {
  if (skills.length === 0 && !suffix) return null
  return (
    <div className="flex flex-wrap items-center justify-start gap-1.5 pb-2">
      <SkillChips names={skills} onRemove={onRemoveSkill} />
      {suffix && (
        <span
          className="flex min-w-0 max-w-[240px] items-center gap-1.5 rounded-full bg-layer-2 px-3 py-1 text-sm text-fg-secondary"
          data-suffix={suffix.id}
        >
          <MessageSquarePlus className="size-3.5 shrink-0" />
          <span className="truncate font-medium">{suffix.name}</span>
          <span className="shrink-0 text-xs text-fg-tertiary">· {suffix.scope === 'once' ? 'this message' : 'every message'}</span>
          <button type="button" aria-label="Turn off prompt suffix" onClick={onClearSuffix} className="ml-1 shrink-0 hover:text-foreground">
            <X className="size-3" />
          </button>
        </span>
      )}
    </div>
  )
}
