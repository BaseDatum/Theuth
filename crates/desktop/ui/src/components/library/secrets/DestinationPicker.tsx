import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { queryKeys, useApi } from '@/lib/rpc'
import type { DestinationView } from '@/rpc/bindings/method.secrets.output'

const SELECT = 'h-7 rounded-md border border-border-subtle bg-transparent px-1.5 text-xs'
const BW_FIELDS = ['password', 'username', 'totp', 'notes', 'uri'] as const

/**
 * §9.2 picker under the request card's Destination strip. AWS: name, KMS key, plain / JSON. Bitwarden: organisation →
 * collection, or folder for personal items; item type; which Bitwarden field each form field fills; new entry or
 * replace an existing one (search over the connection's items). Keep a local copy applies to both.
 */
export function DestinationPicker({ value, fields, onChange, disabled }: { value: DestinationView; fields: readonly { name: string; label: string }[]; onChange: (d: DestinationView) => void; disabled: boolean }) {
  const api = useApi()
  const [search, setSearch] = useState('')
  const replacing = value.replace !== null
  const items = useQuery({
    enabled: replacing && value.status.state === 'ready',
    queryKey: queryKeys.secretExternalList(value.connection, search, false),
    queryFn: () => api.list_external_secrets(value.connection, { query: search || null, cursor: null, deleted: false }),
  })
  const set = (patch: Partial<DestinationView>) => onChange({ ...value, ...patch })
  const text = (key: 'name' | 'kms_key_id' | 'org' | 'collection' | 'folder' | 'uri', label: string, placeholder: string) => (
    <Input
      aria-label={label}
      placeholder={placeholder}
      className="h-7 text-xs"
      value={value[key] ?? ''}
      onChange={(e) => set({ [key]: key === 'name' ? e.target.value : e.target.value || null })}
      spellCheck={false}
    />
  )
  return (
    <fieldset className="flex flex-col gap-1.5 rounded-md border border-border-subtle p-2" disabled={disabled}>
      <ToggleGroup type="single" variant="outline" size="sm" aria-label="New or replace" value={replacing ? 'replace' : 'new'} onValueChange={(m) => m && set({ replace: m === 'replace' ? (value.replace ?? '') : null })}>
        <ToggleGroupItem value="new">Create new</ToggleGroupItem>
        <ToggleGroupItem value="replace">Replace existing</ToggleGroupItem>
      </ToggleGroup>
      {replacing ? (
        <>
          <Input aria-label="Search entries" placeholder="Search the connection's entries" className="h-7 text-xs" value={search} onChange={(e) => setSearch(e.target.value)} />
          <select aria-label="Entry to replace" className={SELECT} value={value.replace ?? ''} onChange={(e) => set({ replace: e.target.value || '', name: items.data?.entries.find((x) => x.ref === e.target.value)?.name ?? value.name })}>
            <option value="">Choose an entry…</option>
            {items.data?.entries.map((x) => (
              <option key={x.ref} value={x.ref}>
                {x.name} · {x.location}
              </option>
            ))}
          </select>
        </>
      ) : (
        text('name', 'Destination name', value.backend === 'aws' ? 'Secret name (app/ci/token)' : 'Item name')
      )}
      {value.backend === 'aws' ? (
        <>
          {text('kms_key_id', 'KMS key', 'KMS key (default: the connection’s, else aws/secretsmanager)')}
          <ToggleGroup type="single" variant="outline" size="sm" aria-label="Format" value={value.format ?? 'string'} onValueChange={(f) => f && set({ format: f as 'string' | 'json' })}>
            <ToggleGroupItem value="string">Plain</ToggleGroupItem>
            <ToggleGroupItem value="json">JSON</ToggleGroupItem>
          </ToggleGroup>
        </>
      ) : (
        <>
          {text('org', 'Organisation', 'Organisation (empty: personal vault)')}
          {value.org ? text('collection', 'Collection', 'Collection') : text('folder', 'Folder', 'Folder')}
          <select aria-label="Item type" className={SELECT} value={value.item ?? 'login'} onChange={(e) => set({ item: e.target.value as 'login' | 'secure_note' })}>
            <option value="login">Login</option>
            <option value="secure_note">Secure note</option>
          </select>
          {(value.item ?? 'login') === 'login' && text('uri', 'Login URI', 'URI (optional)')}
          {fields.map((f) => {
            const target = value.fields?.[f.name] ?? 'password'
            const custom = target.startsWith('field:')
            return (
              <div key={f.name} className="flex items-center gap-2 text-xs">
                <span className="min-w-0 flex-1 truncate">{f.label}</span>
                <select
                  aria-label={`Bitwarden field for ${f.name}`}
                  className={SELECT}
                  value={custom ? 'field:' : target}
                  onChange={(e) => set({ fields: { ...value.fields, [f.name]: e.target.value === 'field:' ? `field:${f.name}` : e.target.value } })}
                >
                  {BW_FIELDS.map((b) => (
                    <option key={b} value={b}>
                      {b}
                    </option>
                  ))}
                  <option value="field:">custom field</option>
                </select>
                {custom && (
                  <Input
                    aria-label={`custom field name for ${f.name}`}
                    className="h-7 w-32 text-xs"
                    value={target.slice('field:'.length)}
                    onChange={(e) => set({ fields: { ...value.fields, [f.name]: `field:${e.target.value}` } })}
                  />
                )}
              </div>
            )
          })}
        </>
      )}
      <label className="flex items-center justify-between gap-2 text-xs">
        Also keep a local copy
        <Switch size="sm" aria-label="Also keep a local copy" checked={value.keep_local} onCheckedChange={(keep_local) => set({ keep_local })} />
      </label>
    </fieldset>
  )
}
