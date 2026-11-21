import { Check, ChevronLeft, ChevronRight, Sparkles, X } from 'lucide-react'
import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
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

/** One chip per parked skill, in send order, each with its own ✕. */
export function SkillChips({ names, onRemove }: { names: readonly string[]; onRemove: (name: string) => void }) {
  if (names.length === 0) return null
  return (
    <div className="flex flex-wrap justify-end gap-1.5 pb-2">
      {names.map((name) => (
        <span key={name} data-skill-chip={name} className="flex items-center gap-1.5 rounded-full bg-layer-2 px-3 py-1 text-sm text-fg-secondary">
          <Sparkles className="size-3.5" />
          <span className="font-medium">{name}</span>
          <button type="button" aria-label={`Remove skill ${name}`} onClick={() => onRemove(name)} className="ml-1 hover:text-foreground">
            <X className="size-3" />
          </button>
        </span>
      ))}
    </div>
  )
}
