import { useQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { open as pickFile } from '@tauri-apps/plugin-dialog'
import { useState } from 'react'
import { errorText } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { notify } from '@/lib/notify'
import { copySecret } from '@/lib/secret-clipboard'
import { isTauri } from '@/lib/tauri'
import { queryKeys, useApi } from '@/lib/rpc'
import { useQuerySupport } from '@/rpc/capabilities'
import type { Api } from '@/rpc/bindings/client'
import type { SendRecord } from '@/rpc/bindings/method.secrets.output'
import { bytesLabel, fileFits, premiumRefusal } from './labels'
import { EXTERNAL_DEFAULTS, type SendArgs, type SendPassword } from './types'

/** Copy a Send's link (`copy_send_link`) to the clipboard, cleared after 60 s while still there; never rendered. */
export async function copySendLink(api: Api, id: string): Promise<void> {
  try {
    await copySecret(await api.copy_send_link(id))
    notify.success('Link copied; the clipboard clears in 60 s unless you copy something else')
  } catch (e) {
    notify.error(errorText(e))
  }
}

/**
 * What the dialog sends. `text`: a local vault NAME or a `bw/…#field` / `aws/…[#key]` ref (never a value). `file`: an
 * absolute path on the engine (the native picker), `artifact:<id>` or a Bitwarden attachment ref; `bytes` when known.
 */
export type SendSource = { kind: 'text'; source: string; label: string } | { kind: 'file'; file: string | null; label: string; bytes: number | null }

type PasswordMode = 'none' | 'from' | 'ask'

/**
 * §12.5 Send dialog: connection, name, text or file (with the file name the recipient sees), expires (24 h), delete,
 * max opens (1), password (none / from a vault secret / typed, masked), hide email (on), notes. Create calls the owner
 * RPC `create_send`; the user is the actor, so there is no card. File mode is off when the file is over
 * `send_max_file_mb`. A Premium refusal of a text-capable Send offers "Send as text instead". The created Send shows
 * Copy link once; the link goes to the clipboard and is never rendered.
 */
export function SendDialog({ initial, open, onOpenChange }: { initial: SendSource; open: boolean; onOpenChange: (open: boolean) => void }) {
  const api = useApi()
  const can = useQuerySupport(['list_secret_connections', 'create_send', 'copy_send_link'])
  const conns = useQuery({ enabled: can && open, queryKey: queryKeys.secretConnections, queryFn: () => api.list_secret_connections() })
  const limits = useQuery({ enabled: can && open, queryKey: queryKeys.secretExternalSettings, queryFn: () => api.get_external_settings(), retry: false })
  const bw = (conns.data ?? []).filter((c) => c.backend === 'bw')
  const maxMb = limits.data?.send_max_file_mb ?? EXTERNAL_DEFAULTS.send_max_file_mb
  const [conn, setConn] = useState<string | null>(null)
  const [source, setSource] = useState<SendSource>(initial)
  const [asFile, setAsFile] = useState<string | null>(null)
  const [fileName, setFileName] = useState('')
  const [name, setName] = useState('')
  const [expires, setExpires] = useState('24h')
  const [del, setDel] = useState('')
  const [maxOpens, setMaxOpens] = useState('1')
  const [pwMode, setPwMode] = useState<PasswordMode>('none')
  const [pwFrom, setPwFrom] = useState('')
  const [hideEmail, setHideEmail] = useState(true)
  const [notes, setNotes] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [created, setCreated] = useState<SendRecord | null>(null)
  const target = conn ?? bw[0]?.conn ?? null
  const fileBytes = source.kind === 'file' ? source.bytes : null
  const tooBig = !fileFits(fileBytes, maxMb)
  // File mode is off when the file the dialog opened with is over the limit.
  const initialTooBig = initial.kind === 'file' && !fileFits(initial.bytes, maxMb)
  const opens = Number(maxOpens)
  // `ask`: after Create, a masked one-field card asks for the password (§12.3 `SendPassword::Ask`); the dialog never holds it.
  const password = (): SendPassword | null => (pwMode === 'from' ? { from: pwFrom.trim() } : pwMode === 'ask' ? { ask: true } : null)
  const args = (): SendArgs => ({
    source: source.kind === 'text' ? source.source : null,
    file: source.kind === 'file' ? source.file : null,
    as_file: source.kind === 'text' ? asFile : null,
    file_name: fileName.trim() || null,
    name: name.trim() || null,
    expires: expires.trim() || null,
    delete: del.trim() || null,
    max_access_count: Number.isInteger(opens) && opens > 0 ? opens : null,
    password: password(),
    hide_email: hideEmail,
    notes: notes.trim() || null,
  })
  const ready =
    target !== null &&
    (source.kind === 'text' ? source.source : source.file) &&
    !tooBig &&
    (pwMode !== 'from' || pwFrom.trim())
  const create = async (a: SendArgs) => {
    if (!target) return
    setBusy(true)
    setError(null)
    try {
      setCreated(await api.create_send(target, a))
      void conns.refetch()
    } catch (e) {
      setError(errorText(e))
    } finally {
      setBusy(false)
    }
  }
  const retryAsText = error !== null && premiumRefusal(error) && source.kind === 'text' && asFile !== null
  const pick = async () => {
    const path = await pickFile({ multiple: false, directory: false })
    if (typeof path === 'string') setSource({ kind: 'file', file: path, label: path, bytes: null })
  }
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>Send with Bitwarden</DialogTitle>
          <DialogDescription>A one-off encrypted link. Anyone with the link can open it until it expires or is used up.</DialogDescription>
        </DialogHeader>
        {created ? (
          <div className="flex flex-col gap-3 text-sm">
            <div>
              Created {created.kind} Send <span className="font-mono">{created.link_secret ?? created.id}</span> on bw/{created.conn}.
            </div>
            <Button onClick={() => void copySendLink(api, created.id)}>Copy link</Button>
            <div className="text-xs text-fg-tertiary">The link is copied to the clipboard, never shown here.</div>
          </div>
        ) : (
          <div className="flex flex-col gap-3 text-sm">
            {bw.length === 0 && <div className="text-fg-secondary">Add a Bitwarden connection on the Secrets page first.</div>}
            {bw.length > 1 && (
              <label className="flex items-center justify-between gap-2">
                Connection
                <select aria-label="Bitwarden connection" className="h-8 rounded-md border border-border-subtle bg-transparent px-1.5" value={target ?? ''} onChange={(e) => setConn(e.target.value)}>
                  {bw.map((c) => (
                    <option key={c.conn} value={c.conn}>
                      {c.conn} · {c.summary}
                    </option>
                  ))}
                </select>
              </label>
            )}
            <ToggleGroup
              type="single"
              variant="outline"
              size="sm"
              aria-label="Content"
              value={source.kind}
              onValueChange={(k) => {
                if (k === 'text' && initial.kind === 'text') setSource(initial)
                if (k === 'file') setSource(initial.kind === 'file' ? initial : { kind: 'file', file: null, label: '', bytes: null })
              }}
            >
              <ToggleGroupItem value="text" disabled={initial.kind !== 'text'}>
                Text
              </ToggleGroupItem>
              <ToggleGroupItem value="file" disabled={initialTooBig}>
                File
              </ToggleGroupItem>
            </ToggleGroup>
            {source.kind === 'text' ? (
              <>
                <div className="text-fg-secondary">
                  Sends <span className="font-mono">{source.label}</span>; theuth reads it, the agent never does.
                </div>
                <label className="flex items-center justify-between gap-2">
                  Send as a file
                  <Switch aria-label="Send as a file" checked={asFile !== null} onCheckedChange={(on) => setAsFile(on ? '' : null)} />
                </label>
                {asFile !== null && <Input aria-label="file name of the text" placeholder="File name (.env)" value={asFile} onChange={(e) => setAsFile(e.target.value)} />}
              </>
            ) : (
              <>
                <div className="flex items-center gap-2">
                  <span className="min-w-0 flex-1 truncate font-mono text-xs">{source.label || 'No file chosen'}</span>
                  {fileBytes !== null && <span className="text-xs text-fg-tertiary">{bytesLabel(fileBytes)}</span>}
                  {isTauri() && (
                    <Button size="sm" variant="outline" onClick={() => void pick().catch((e) => setError(errorText(e)))}>
                      Choose file…
                    </Button>
                  )}
                </div>
                {tooBig && (
                  <div className="text-xs text-danger">
                    Over the {maxMb} MB limit (Settings › Agent › Secrets › Send file size).
                  </div>
                )}
                <Input aria-label="file name the recipient sees" placeholder="File name the recipient sees (default: the source's own)" value={fileName} onChange={(e) => setFileName(e.target.value)} />
              </>
            )}
            <Input aria-label="Send name" placeholder='Name shown to the recipient (default "Shared from theuth")' value={name} onChange={(e) => setName(e.target.value)} />
            <div className="grid grid-cols-3 gap-2">
              <Input aria-label="expires" placeholder="Expires (24h)" value={expires} onChange={(e) => setExpires(e.target.value)} />
              <Input aria-label="delete after" placeholder="Delete (= expires)" value={del} onChange={(e) => setDel(e.target.value)} />
              <Input aria-label="max opens" type="number" min={1} value={maxOpens} onChange={(e) => setMaxOpens(e.target.value)} />
            </div>
            <ToggleGroup type="single" variant="outline" size="sm" aria-label="Password" value={pwMode} onValueChange={(m) => m && setPwMode(m as PasswordMode)}>
              <ToggleGroupItem value="none">No password</ToggleGroupItem>
              <ToggleGroupItem value="from">From a vault secret</ToggleGroupItem>
              <ToggleGroupItem value="ask">Type one on a card</ToggleGroupItem>
            </ToggleGroup>
            {pwMode === 'from' && <Input aria-label="password secret name" placeholder="Vault secret name" value={pwFrom} onChange={(e) => setPwFrom(e.target.value)} spellCheck={false} />}
            <label className="flex items-center justify-between gap-2">
              Hide my email from the recipient
              <Switch aria-label="Hide my email" checked={hideEmail} onCheckedChange={setHideEmail} />
            </label>
            <Input
              aria-label="Send notes"
              placeholder="Private notes (never the secret)"
              value={notes}
              onChange={(e) => setNotes(e.target.value)}
            />
            {error && <div className="text-xs text-danger">{error}</div>}
            {busy && (
              <div role="status" className="text-xs text-fg-secondary">
                {pwMode === 'ask' ? (
                  <>
                    A card asks for the Send's password in your{' '}
                    <Link to="/attention" className="underline">
                      inbox
                    </Link>
                    ; this dialog waits for it. Once answered, the Send also shows under the connection's Sends.
                  </>
                ) : source.kind === 'file' ? (
                  `Encrypting and uploading ${source.label || 'the file'}${fileBytes !== null ? ` (${bytesLabel(fileBytes)})` : ''}…`
                ) : (
                  'Creating the Send…'
                )}
              </div>
            )}
          </div>
        )}
        <DialogFooter>
          {created ? (
            <Button variant="outline" onClick={() => onOpenChange(false)}>
              Done
            </Button>
          ) : (
            <>
              {retryAsText && (
                <Button
                  variant="outline"
                  disabled={busy}
                  onClick={() => {
                    setAsFile(null)
                    void create({ ...args(), as_file: null })
                  }}
                >
                  Send as text instead
                </Button>
              )}
              <Button disabled={busy || !ready} onClick={() => void create(args())}>
                {busy ? 'Creating…' : 'Create Send'}
              </Button>
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
