import { useQuerySupport } from '@/rpc/capabilities'
// 158 §11.1 (W4): Settings › Meetings › Templates — every `<data_dir>/meetings/templates/<slug>.md` as the engine
// parsed it (`meeting_templates`), Edit opens the file itself (`body`, saved verbatim), New starts from one Summary
// section, Save → `meeting_template_put(slug, body)`, Delete → `meeting_template_delete` (disabled for built-ins),
// an edited built-in gets "Reset to built-in" → `meeting_template_reset`. The Defaults row's Template select and
// the start form read the same query, so a save shows up there at once.

import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'
import { CharterEditor } from '@/components/agents/CharterEditor'
import { Empty, Row, Section, errorText, useAction } from '@/components/settings/shared'
import type { TabProps } from '@/components/settings/types'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { NEW_TEMPLATE, slugify, templateBody } from '@/lib/meeting-templates'
import { queryKeys, useApi } from '@/lib/rpc'

/** The editor's state: `isNew` keeps the slug editable (typed freely, `slugify`d on save). */
export interface Draft {
  slug: string
  isNew: boolean
  body: string
}

/** Meetings › Templates (188: a whole tab; it takes no `focus`). `initial` opens the editor at mount (tests; a "New template…" link could pass one). */
export function MeetingTemplates({ initial = null }: Partial<TabProps> & { initial?: Draft | null }) {
  const canQuery1 = useQuerySupport(["meeting_templates"])
  const api = useApi()
  const templates = useQuery({ enabled: canQuery1, queryKey: queryKeys.meetingTemplates, queryFn: () => api.meeting_templates() })
  const [draft, setDraft] = useState<Draft | null>(initial)
  const put = useAction((d: Draft) => api.meeting_template_put(d.isNew ? slugify(d.slug) : d.slug, d.body), {
    keys: [queryKeys.meetingTemplates],
    onSuccess: () => setDraft(null),
  })
  const del = useAction((slug: string) => api.meeting_template_delete(slug), { keys: [queryKeys.meetingTemplates] })
  const reset = useAction((slug: string) => api.meeting_template_reset(slug), { keys: [queryKeys.meetingTemplates] })
  const slug = draft ? (draft.isNew ? slugify(draft.slug) : draft.slug) : ''
  const taken = draft?.isNew && templates.data?.some((t) => t.slug === slug)
  return (
    <Section
      title="Templates"
      actions={
        <Button variant="outline" size="sm" disabled={!!draft} onClick={() => setDraft({ slug: '', isNew: true, body: templateBody(NEW_TEMPLATE) })}>
          New template
        </Button>
      }
    >
      {templates.isError && <Empty>{errorText(templates.error)}</Empty>}
      {templates.data?.length === 0 && <Empty>No templates on file. The built-ins appear once the engine has written them.</Empty>}
      {templates.data?.map((t) => (
        <Row
          key={t.slug}
          label={
            <span className="flex items-center gap-2">
              {t.name}
              {t.builtin && <Badge variant="outline">built-in</Badge>}
            </span>
          }
          description={`${t.description || `${t.slug}.md`} · ${t.sections.map((s) => s.title).join(', ')}`}
        >
          <Button variant="outline" size="sm" aria-label={`Edit ${t.name}`} disabled={!!draft} onClick={() => setDraft({ slug: t.slug, isNew: false, body: t.body })}>
            Edit
          </Button>
          {t.builtin && t.edited && (
            <Button variant="outline" size="sm" aria-label={`Reset ${t.name} to built-in`} disabled={reset.isPending} onClick={() => reset.mutate(t.slug)}>
              Reset to built-in
            </Button>
          )}
          <Button
            variant="outline"
            size="sm"
            aria-label={`Delete ${t.name}`}
            disabled={t.builtin || del.isPending}
            title={t.builtin ? 'Built-in templates cannot be deleted' : undefined}
            onClick={() => del.mutate(t.slug)}
          >
            Delete
          </Button>
        </Row>
      ))}
      {draft && (
        <div className="flex flex-col gap-2 py-4">
          {draft.isNew ? (
            <div className="flex items-center gap-2 text-[13px] max-lg:flex-wrap">
              <Input className="h-8 w-64" placeholder="Name of the file" aria-label="Template slug" value={draft.slug} onChange={(e) => setDraft({ ...draft, slug: e.target.value })} />
              <span className="text-fg-tertiary">{slug ? `${slug}.md` : 'lowercase words joined by -'}</span>
              {taken && <span className="text-destructive">already exists</span>}
            </div>
          ) : (
            <div className="text-[13px] text-fg-tertiary">{draft.slug}.md</div>
          )}
          <CharterEditor value={draft.body} onChange={(body) => setDraft({ ...draft, body })} className="h-64" />
          <div className="flex justify-end gap-2">
            <Button variant="ghost" size="sm" onClick={() => setDraft(null)}>
              Cancel
            </Button>
            <Button size="sm" disabled={!slug || !!taken || !draft.body.trim() || put.isPending} onClick={() => put.mutate(draft)}>
              Save
            </Button>
          </div>
        </div>
      )}
    </Section>
  )
}
