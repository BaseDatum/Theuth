import { useQuery } from '@tanstack/react-query'
import { Trash2 } from 'lucide-react'
import { useRef, useState } from 'react'
import { toast } from 'sonner'
import { Column, Page } from '@/components/library/Page'
import { Empty, Row, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { queryKeys, useApi } from '@/lib/rpc'
import type { SecretInfo } from '@/rpc/bindings/SecretInfo'

const EMPTY_FORM = { name: '', value: '', note: '' }

/** `2026-09-16T10:02:11Z` → the user's locale, minutes precision. */
function when(iso: string): string {
  const d = new Date(iso)
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })
}

/**
 * `/secrets` (37 §6): names, notes and versions from `list_secrets`; a value is typed once into the password
 * field, sent by `set_secret`, and never comes back — no length, no preview. The form doubles as Replace
 * (prefilled name = a new version of that name).
 */
export function Secrets() {
  const api = useApi()
  const list = useQuery({ queryKey: queryKeys.secrets, queryFn: () => api.list_secrets() })
  const [form, setForm] = useState(EMPTY_FORM)
  const [history, setHistory] = useState<string | null>(null)
  const valueRef = useRef<HTMLInputElement>(null)
  const add = useAction((v: typeof form) => api.set_secret(v.name, v.value, v.note || null), {
    keys: [queryKeys.secrets],
    onSuccess: (s) => {
      setForm(EMPTY_FORM)
      toast.success(`Stored ${s.name} v${s.current}`)
    },
  })
  const restore = useAction((v: { name: string; back: number }) => api.restore_secret(v.name, v.back), {
    keys: [queryKeys.secrets],
    onSuccess: (s) => toast.success(`${s.name}: v${s.current} restored`),
  })
  const remove = useAction((name: string) => api.delete_secret(name), { keys: [queryKeys.secrets] })
  const ready = form.name.trim() && form.value
  const replace = (s: SecretInfo) => {
    setForm({ name: s.name, value: '', note: s.note })
    valueRef.current?.focus()
  }

  return (
    <Page
      title="Secrets"
      tabs="library"
      actions={list.data && <span className="px-2 text-[13px] text-fg-tertiary">key: {list.data.key_store}</span>}
    >
      <Column>
        {list.data?.secrets.length === 0 && (
          <Empty>
            No secret yet. A secret is a value the harness can hand to tools by name — the model references it as a
            placeholder and never sees the value. Add one below.
          </Empty>
        )}
        {list.data?.secrets.map((s) => (
          <div key={s.name} className="border-b border-border-subtle last:border-0">
            <Row
              label={<span className="font-mono">{s.name}</span>}
              description={
                <>
                  v{s.current} · {s.versions.length} kept{s.note && <> · {s.note}</>} · {when(s.updated)}
                </>
              }
            >
              <Button variant="outline" size="sm" onClick={() => replace(s)}>
                Replace
              </Button>
              <Button variant="outline" size="sm" onClick={() => setHistory(history === s.name ? null : s.name)}>
                History
              </Button>
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label={`Delete ${s.name}`}
                disabled={remove.isPending}
                onClick={() => confirm(`Delete ${s.name} and all ${s.versions.length} versions?`) && remove.mutate(s.name)}
              >
                <Trash2 />
              </Button>
            </Row>
            {history === s.name && (
              <div className="mb-3 ml-4 flex flex-col gap-1 text-[13px] text-fg-secondary">
                {s.versions.map((v, i) => (
                  <div key={v.v} className="flex items-center justify-between gap-4">
                    <span>
                      v{v.v} · {when(v.created)} · {v.source}
                      {i === 0 && <span className="text-fg-tertiary"> · current</span>}
                    </span>
                    {i > 0 && (
                      <Button
                        variant="ghost"
                        size="sm"
                        disabled={restore.isPending}
                        onClick={() => restore.mutate({ name: s.name, back: i })}
                      >
                        Restore
                      </Button>
                    )}
                  </div>
                ))}
              </div>
            )}
          </div>
        ))}
        <form
          className="mt-4 grid grid-cols-[1fr_2fr_2fr_auto] gap-2"
          onSubmit={(e) => {
            e.preventDefault()
            if (ready) add.mutate({ name: form.name.trim(), value: form.value, note: form.note.trim() })
          }}
        >
          <Input
            value={form.name}
            onChange={(e) => setForm({ ...form, name: e.target.value })}
            placeholder="Name"
            aria-label="Secret name"
            spellCheck={false}
          />
          <Input
            ref={valueRef}
            type="password"
            value={form.value}
            onChange={(e) => setForm({ ...form, value: e.target.value })}
            placeholder="Value"
            aria-label="value"
            autoComplete="off"
          />
          <Input
            value={form.note}
            onChange={(e) => setForm({ ...form, note: e.target.value })}
            placeholder="Note (what it is for)"
            aria-label="Note"
          />
          <Button type="submit" variant="outline" disabled={!ready || add.isPending}>
            {list.data?.secrets.some((s) => s.name === form.name.trim()) ? 'Replace' : 'Add'}
          </Button>
        </form>
      </Column>
    </Page>
  )
}
