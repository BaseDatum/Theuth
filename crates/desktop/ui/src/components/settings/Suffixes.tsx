// #3 Settings › Prompt suffixes: the named suffixes the composer's `+` › Add prompt suffix lists. On an engine that
// keeps them, shared by every machine of the mesh and every device connected to it; else local to this device
// (`lib/suffixes`). New / Edit open an inline form (name + multiline text), Delete removes the suffix and turns it
// off in every chat that had it on. Same page on desktop, iPad and phone (the settings page lays it out).

import { useState } from 'react'
import { Empty, Row, Section } from '@/components/settings/shared'
import type { TabProps } from '@/components/settings/types'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { useSuffixes } from '@/lib/suffixes'

/** The form's state: `id` null for a new suffix. */
type Form = { id: string | null; name: string; text: string }

export function Suffixes(_: Partial<TabProps>) {
  const list = useSuffixes((s) => s.list)
  const [form, setForm] = useState<Form | null>(null)
  const save = () => {
    if (!form) return
    const name = form.name.trim()
    const store = useSuffixes.getState()
    if (form.id === null) store.add(name, form.text)
    else store.update(form.id, { name, text: form.text })
    setForm(null)
  }
  return (
    <Section
      title="Prompts"
      actions={
        <Button variant="outline" size="sm" disabled={!!form} onClick={() => setForm({ id: null, name: '', text: '' })}>
          New prompt
        </Button>
      }
    >
      <p className="py-2 text-[13px] text-fg-secondary">
        Saved text you can insert into a message by typing &amp; in the composer. You can also attach one from the
        composer’s + menu for this message or for every message until you turn it off.
      </p>
      {list.length === 0 && !form && <Empty>No prompts yet.</Empty>}
      {list.map((s) =>
        form?.id === s.id ? null : (
          <Row key={s.id} label={s.name} description={s.text.length > 120 ? `${s.text.slice(0, 120)}…` : s.text}>
            <Button variant="outline" size="sm" aria-label={`Edit ${s.name}`} disabled={!!form} onClick={() => setForm({ id: s.id, name: s.name, text: s.text })}>
              Edit
            </Button>
            <Button variant="outline" size="sm" aria-label={`Delete ${s.name}`} onClick={() => useSuffixes.getState().remove(s.id)}>
              Delete
            </Button>
          </Row>
        ),
      )}
      {form && (
        <form
          className="flex flex-col gap-2 py-3"
          onSubmit={(e) => {
            e.preventDefault()
            save()
          }}
        >
          <Input aria-label="Prompt name" placeholder="Name" value={form.name} autoFocus onChange={(e) => setForm({ ...form, name: e.target.value })} />
          <Textarea
            aria-label="Prompt text"
            placeholder="Prompt text"
            rows={5}
            value={form.text}
            onChange={(e) => setForm({ ...form, text: e.target.value })}
            className="min-h-28"
          />
          <div className="flex justify-end gap-2">
            <Button type="button" variant="ghost" size="sm" onClick={() => setForm(null)}>
              Cancel
            </Button>
            <Button type="submit" size="sm" disabled={!form.name.trim() || !form.text.trim()}>
              Save
            </Button>
          </div>
        </form>
      )}
    </Section>
  )
}
