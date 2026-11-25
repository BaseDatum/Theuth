// Settings › Prompt › System: inspect, filter, edit, and reset every section assembled into the system prompt.
import { useQuery } from '@tanstack/react-query'
import { ChevronDown, ChevronRight } from 'lucide-react'
import { useState } from 'react'
import { CharterEditor } from '@/components/agents/CharterEditor'
import { Empty, Section, errorText, useAction } from '@/components/settings/shared'
import type { TabProps } from '@/components/settings/types'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { notify as toast } from '@/lib/notify'
import { queryKeys, useApi } from '@/lib/rpc'
import { useQuerySupport } from '@/rpc/capabilities'

export interface PromptSection {
  id: string
  group: string
  label: string
  description: string
  content: string
  defaultContent: string
  customized: boolean
}

type GroupFilter = 'all' | 'core' | 'plugin' | 'modified'

function matchesQuery(section: PromptSection, needle: string): boolean {
  if (!needle) return true
  return (
    section.label.toLowerCase().includes(needle) ||
    section.id.toLowerCase().includes(needle) ||
    section.description.toLowerCase().includes(needle) ||
    section.content.toLowerCase().includes(needle)
  )
}

function PromptSectionRow({
  section,
  open,
  draft,
  dirty,
  savePending,
  resetPending,
  onToggle,
  onChange,
  onRevert,
  onSave,
  onReset,
}: {
  section: PromptSection
  open: boolean
  draft: string
  dirty: boolean
  savePending: boolean
  resetPending: boolean
  onToggle: () => void
  onChange: (value: string) => void
  onRevert: () => void
  onSave: () => void
  onReset: () => void
}) {
  return (
    <div className="border-b border-border-subtle last:border-0">
      <button
        type="button"
        aria-expanded={open}
        onClick={onToggle}
        className="flex w-full items-center justify-between gap-3 py-3 text-left outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
      >
        <span className="flex min-w-0 flex-1 flex-wrap items-center gap-x-2 gap-y-1">
          {open ? (
            <ChevronDown className="size-4 shrink-0 text-fg-tertiary" />
          ) : (
            <ChevronRight className="size-4 shrink-0 text-fg-tertiary" />
          )}
          <span className="text-sm font-medium">{section.label}</span>
          <span className="rounded bg-layer-2 px-1.5 py-0.5 font-mono text-[11px] text-fg-tertiary">{section.id}</span>
          {section.customized && (
            <Badge variant="secondary">{section.content.trim() === '' ? 'Empty' : 'Modified'}</Badge>
          )}
          {dirty && <Badge variant="outline">Unsaved</Badge>}
        </span>
        <span className="shrink-0 text-[12px] tabular-nums text-fg-tertiary">
          {draft.length.toLocaleString('en-US')} chars
        </span>
      </button>
      {open && (
        <div className="flex flex-col gap-3 pb-4 pl-6">
          <p className="text-[12px] text-fg-secondary">{section.description}</p>
          <CharterEditor value={draft} onChange={onChange} className="h-64" />
          <div className="flex flex-wrap items-center justify-between gap-2">
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={!section.customized || resetPending}
              onClick={onReset}
            >
              Reset to default
            </Button>
            <div className="flex items-center gap-2">
              {dirty && (
                <Button type="button" variant="ghost" size="sm" disabled={savePending} onClick={onRevert}>
                  Revert
                </Button>
              )}
              <Button type="button" size="sm" disabled={!dirty || savePending} onClick={onSave}>
                Save
              </Button>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}

export function SystemPrompt(_: Partial<TabProps>) {
  const canQuery = useQuerySupport(['list_prompt_sections', 'set_prompt_section', 'reset_prompt_section'])
  const api = useApi()
  const sections = useQuery({
    enabled: canQuery,
    queryKey: queryKeys.promptSections,
    queryFn: () => api.list_prompt_sections(),
  })

  const [query, setQuery] = useState('')
  const [filter, setFilter] = useState<GroupFilter>('all')
  const [expanded, setExpanded] = useState<Record<string, boolean>>({})
  const [drafts, setDrafts] = useState<Record<string, string>>({})

  const clearDraft = (id: string) =>
    setDrafts((prev) => {
      if (!(id in prev)) return prev
      const next = { ...prev }
      delete next[id]
      return next
    })

  const save = useAction(
    (vars: { id: string; content: string }) => api.set_prompt_section(vars.id, vars.content),
    {
      keys: [queryKeys.promptSections],
      onSuccess: (_, vars) => {
        clearDraft(vars.id)
        toast.success('Saved')
      },
    },
  )

  const reset = useAction((id: string) => api.reset_prompt_section(id), {
    keys: [queryKeys.promptSections],
    onSuccess: (_, id) => {
      clearDraft(id)
      toast.success('Reset to default')
    },
  })

  const list: readonly PromptSection[] = sections.data ?? []
  const modifiedCount = list.filter((s) => s.customized).length
  const needle = query.trim().toLowerCase()

  const filtered = list.filter((s) => {
    if (filter === 'core' && s.group !== 'core') return false
    if (filter === 'plugin' && s.group !== 'plugin') return false
    if (filter === 'modified' && !s.customized) return false
    return matchesQuery(s, needle)
  })

  const coreSections = filtered.filter((s) => s.group === 'core')
  const pluginSections = filtered.filter((s) => s.group === 'plugin')

  const filterOptions: { id: GroupFilter; label: string }[] = [
    { id: 'all', label: 'All' },
    { id: 'core', label: 'Core' },
    { id: 'plugin', label: 'Plugins' },
    { id: 'modified', label: `Modified (${modifiedCount})` },
  ]

  const renderRows = (items: readonly PromptSection[]) =>
    items.map((section) => {
      const hasDraft = Object.prototype.hasOwnProperty.call(drafts, section.id)
      const draft = hasDraft ? (drafts[section.id] ?? section.content) : section.content
      const dirty = draft !== section.content
      return (
        <PromptSectionRow
          key={section.id}
          section={section}
          open={Boolean(expanded[section.id])}
          draft={draft}
          dirty={dirty}
          savePending={save.isPending}
          resetPending={reset.isPending}
          onToggle={() => setExpanded((prev) => ({ ...prev, [section.id]: !prev[section.id] }))}
          onChange={(value) => {
            if (value === section.content) clearDraft(section.id)
            else setDrafts((prev) => ({ ...prev, [section.id]: value }))
          }}
          onRevert={() => clearDraft(section.id)}
          onSave={() => save.mutate({ id: section.id, content: draft })}
          onReset={() => reset.mutate(section.id)}
        />
      )
    })

  return (
    <div>
      <div className="mb-6 flex flex-col gap-3">
        <p className="text-[13px] text-fg-secondary">
          These sections are assembled into the system prompt on each turn. Edits apply immediately on the next turn
          without restarting; saving empty text suppresses a section.
        </p>
        <div className="flex flex-wrap items-center gap-2">
          <Input
            placeholder="Filter sections..."
            aria-label="Filter sections"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            className="h-8 min-w-48 flex-1"
          />
          <div className="flex flex-wrap items-center gap-1" role="group" aria-label="Filter by group">
            {filterOptions.map((opt) => (
              <Button
                key={opt.id}
                type="button"
                variant={filter === opt.id ? 'secondary' : 'outline'}
                size="sm"
                aria-pressed={filter === opt.id}
                onClick={() => setFilter(opt.id)}
              >
                {opt.label}
              </Button>
            ))}
          </div>
        </div>
      </div>

      {sections.isError && <Empty>{errorText(sections.error)}</Empty>}
      {!sections.isError && sections.isLoading && <Empty>Loading prompt sections...</Empty>}
      {!sections.isError && !sections.isLoading && filtered.length === 0 && (
        <Empty>{list.length === 0 ? 'No prompt sections available.' : 'No prompt sections match the current filter.'}</Empty>
      )}

      {coreSections.length > 0 && <Section title="System & core">{renderRows(coreSections)}</Section>}
      {pluginSections.length > 0 && <Section title="Plugins">{renderRows(pluginSections)}</Section>}
    </div>
  )
}
