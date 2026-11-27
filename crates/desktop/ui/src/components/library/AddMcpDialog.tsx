import { CheckCircle2, ChevronDown, ChevronRight, ExternalLink, KeyRound, LoaderCircle, Plus, PlugZap, Trash2, XCircle } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, DialogTrigger } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Textarea } from '@/components/ui/textarea'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { errorText } from '@/components/settings/shared'
import { gitUrl, mcpEntry } from '@/lib/mcpEntry'
import { notify as toast } from '@/lib/notify'
import { isIos } from '@/lib/platform'
import { queryKeys, useApi } from '@/lib/rpc'
import { closeAuthBrowser, onAuthBrowserDismissed, openAuthBrowser } from '@/lib/tauri'
import { LOCAL, useEngines } from '@/link/engines'
import { useQuerySupport } from '@/rpc/capabilities'
import type { McpServer } from '@/rpc/bindings/method.mcp_2dserver.output'
import { useConnection } from '@/store/connection'
import { on as onEvent } from '@/store/sessions'

export type McpTestStatus = 'connected' | 'auth_required' | 'failed'
export type McpTestResult = {
  status: McpTestStatus
  tools: string[]
  error: string | null
}

export type KvRow = { id: number; key: string; value: string }
export type AuthMode = 'oauth' | 'bearer' | 'headers'
export type TransportMode = 'http' | 'stdio'
export type EditorMode = 'form' | 'json'

export type McpFormState = {
  name: string
  transport: TransportMode
  url: string
  authMode: AuthMode
  bearerToken: string
  headers: KvRow[]
  oauthClientId: string
  oauthClientSecret: string
  oauthScopes: string
  oauthRedirectUri: string
  command: string
  args: string
  cwd: string
  env: KvRow[]
}

const NAME_RE = /^[a-z0-9][a-z0-9._-]*$/

export function emptyForm(): McpFormState {
  return {
    name: '',
    transport: 'http',
    url: '',
    authMode: 'oauth',
    bearerToken: '',
    headers: [{ id: 1, key: '', value: '' }],
    oauthClientId: '',
    oauthClientSecret: '',
    oauthScopes: '',
    oauthRedirectUri: '',
    command: '',
    args: '',
    cwd: '',
    env: [{ id: 1, key: '', value: '' }],
  }
}

/** Split shell-like arguments (supports single/double quotes or newline-separated args). */
export function parseArgs(raw: string): string[] {
  const trimmed = raw.trim()
  if (!trimmed) return []
  if (trimmed.includes('\n')) {
    return trimmed
      .split('\n')
      .map((l) => l.trim())
      .filter(Boolean)
  }
  const out: string[] = []
  let cur = ''
  let quote: '"' | "'" | null = null
  for (let i = 0; i < trimmed.length; i++) {
    const ch = trimmed[i]!
    if (quote) {
      if (ch === quote) quote = null
      else cur += ch
    } else if (ch === '"' || ch === "'") {
      quote = ch
    } else if (/\s/.test(ch)) {
      if (cur) {
        out.push(cur)
        cur = ''
      }
    } else {
      cur += ch
    }
  }
  if (cur) out.push(cur)
  return out
}

/** Build a normalized `{ name, entry }` payload from the form state. Throws a clear message if invalid. */
export function formToEntry(form: McpFormState): { name: string; entry: Record<string, unknown> } {
  const name = form.name.trim().toLowerCase()
  if (!name) throw new Error('Enter a server name')
  if (!NAME_RE.test(name)) {
    throw new Error('Server name must start with a letter or digit and use only a-z, 0-9, ., _, -')
  }
  if (form.transport === 'http') {
    const url = form.url.trim()
    if (!url) throw new Error('Enter a server URL')
    if (!/^https?:\/\//i.test(url)) throw new Error('Server URL must start with http:// or https://')
    const entry: Record<string, unknown> = { url }
    const headers: Record<string, string> = {}
    if (form.authMode === 'bearer') {
      const tok = form.bearerToken.trim()
      if (tok) headers.Authorization = tok.toLowerCase().startsWith('bearer ') ? tok : `Bearer ${tok}`
    } else if (form.authMode === 'headers') {
      for (const row of form.headers) {
        const k = row.key.trim()
        const v = row.value.trim()
        if (k && v) headers[k] = v
      }
    }
    if (Object.keys(headers).length > 0) entry.headers = headers
    if (form.authMode === 'oauth') {
      const oauth: Record<string, unknown> = {}
      if (form.oauthClientId.trim()) oauth.client_id = form.oauthClientId.trim()
      if (form.oauthClientSecret.trim()) oauth.client_secret = form.oauthClientSecret.trim()
      const scopes = form.oauthScopes
        .split(/[\s,]+/)
        .map((s) => s.trim())
        .filter(Boolean)
      if (scopes.length > 0) oauth.scopes = scopes
      if (form.oauthRedirectUri.trim()) oauth.redirect_uri = form.oauthRedirectUri.trim()
      if (Object.keys(oauth).length > 0) entry.oauth = oauth
    }
    return { name, entry }
  }

  const command = form.command.trim()
  if (!command) throw new Error('Enter a command to run')
  const entry: Record<string, unknown> = { command }
  const args = parseArgs(form.args)
  if (args.length > 0) entry.args = args
  if (form.cwd.trim()) entry.cwd = form.cwd.trim()
  const env: Record<string, string> = {}
  for (const row of form.env) {
    const k = row.key.trim()
    if (k) env[k] = row.value
  }
  if (Object.keys(env).length > 0) entry.env = env
  return { name, entry }
}

/** Parse a `{ name, entry }` pair back into form state (used when switching JSON → Form). */
export function entryToForm(name: string, raw: unknown): McpFormState {
  const base = emptyForm()
  base.name = name
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return base
  const e = raw as Record<string, unknown>
  if (typeof e.url === 'string') {
    base.transport = 'http'
    base.url = e.url
    const hdrs = e.headers && typeof e.headers === 'object' && !Array.isArray(e.headers) ? (e.headers as Record<string, unknown>) : null
    if (hdrs) {
      const entries = Object.entries(hdrs).filter(([, v]) => typeof v === 'string') as [string, string][]
      const authEntry = entries.find(([k]) => k.toLowerCase() === 'authorization')
      if (entries.length === 1 && authEntry && /^bearer\s+/i.test(authEntry[1])) {
        base.authMode = 'bearer'
        base.bearerToken = authEntry[1].replace(/^bearer\s+/i, '')
      } else if (entries.length > 0) {
        base.authMode = 'headers'
        base.headers = entries.map(([key, value], idx) => ({ id: idx + 1, key, value }))
      }
    }
    const oauth = e.oauth && typeof e.oauth === 'object' && !Array.isArray(e.oauth) ? (e.oauth as Record<string, unknown>) : null
    if (oauth) {
      base.authMode = 'oauth'
      if (typeof oauth.client_id === 'string') base.oauthClientId = oauth.client_id
      if (typeof oauth.client_secret === 'string') base.oauthClientSecret = oauth.client_secret
      if (Array.isArray(oauth.scopes)) base.oauthScopes = oauth.scopes.filter((s): s is string => typeof s === 'string').join(' ')
      if (typeof oauth.redirect_uri === 'string') base.oauthRedirectUri = oauth.redirect_uri
    }
  } else if (typeof e.command === 'string') {
    base.transport = 'stdio'
    base.command = e.command
    if (Array.isArray(e.args)) {
      base.args = e.args.filter((a): a is string => typeof a === 'string').join(' ')
    }
    if (typeof e.cwd === 'string') base.cwd = e.cwd
    const envObj = e.env && typeof e.env === 'object' && !Array.isArray(e.env) ? (e.env as Record<string, unknown>) : null
    if (envObj) {
      const entries = Object.entries(envObj).filter(([, v]) => typeof v === 'string') as [string, string][]
      if (entries.length > 0) base.env = entries.map(([key, value], idx) => ({ id: idx + 1, key, value }))
    }
  }
  return base
}

export function formatServerJson(form: McpFormState): string {
  const name = form.name.trim().toLowerCase() || 'my-server'
  try {
    const { name: n, entry } = formToEntry({ ...form, name })
    return JSON.stringify({ [n]: entry }, null, 2)
  } catch {
    if (form.transport === 'http') {
      return JSON.stringify({ [name]: { url: form.url.trim() || 'https://mcp.example.com/mcp' } }, null, 2)
    }
    return JSON.stringify({ [name]: { command: form.command.trim() || 'npx', args: parseArgs(form.args) } }, null, 2)
  }
}

type ProbeState =
  | { kind: 'idle' }
  | { kind: 'testing' }
  | { kind: 'connected'; tools: string[]; saved: boolean }
  | { kind: 'auth_required'; serverName: string; saved: boolean }
  | { kind: 'signing_in'; serverName: string; url: string | null; local: boolean }
  | { kind: 'failed'; message: string }

export function AddMcpDialog({ session }: { session: string | null }) {
  const api = useApi()
  const qc = useQueryClient()
  const client = useConnection((s) => s.client)
  const canTest = useQuerySupport(['test_mcp_server' as never])
  const canRepo = useQuerySupport(['add_mcp_repo'])
  const ios = isIos()

  const [open, setOpen] = useState(false)
  const [mode, setMode] = useState<EditorMode>('form')
  const [form, setForm] = useState<McpFormState>(emptyForm)
  const [jsonText, setJsonText] = useState('')
  const [oauthOpen, setOauthOpen] = useState(false)
  const [probe, setProbe] = useState<ProbeState>({ kind: 'idle' })
  const [saving, setSaving] = useState(false)
  const nextRowId = useRef(10)
  const probeRef = useRef(probe)
  probeRef.current = probe

  const invalidate = () => {
    void qc.invalidateQueries({ queryKey: queryKeys.mcp(session) })
  }

  const resetAll = () => {
    setForm(emptyForm())
    setJsonText('')
    setOauthOpen(false)
    setProbe({ kind: 'idle' })
    setSaving(false)
    setMode('form')
  }

  // Watch OAuth completion while signing in inside the dialog
  useEffect(() => {
    if (!open) return
    const offProgress = onEvent('tools.mcp-progress@1', 'McpProgress', (e) => {
      const cur = probeRef.current
      if (cur.kind !== 'signing_in') return
      if (!e.connected.includes(cur.serverName)) return
      if (ios && cur.local) void closeAuthBrowser()
      invalidate()
      void api
        .list_mcp(session)
        .then((list) => {
          const found = list.find((x) => x.name === cur.serverName)
          const tools = found?.tools.map((t) => t.name) ?? []
          setProbe({ kind: 'connected', tools, saved: true })
          toast.success(`Connected ${cur.serverName}`)
        })
        .catch(() => {
          setProbe({ kind: 'connected', tools: [], saved: true })
          toast.success(`Connected ${cur.serverName}`)
        })
    })
    const offDismiss = ios
      ? onAuthBrowserDismissed(() => {
          const cur = probeRef.current
          if (cur.kind !== 'signing_in' || !cur.local) return
          setProbe({ kind: 'auth_required', serverName: cur.serverName, saved: true })
        })
      : Promise.resolve(() => {})
    return () => {
      offProgress()
      void offDismiss.then((fn) => fn())
    }
  }, [open, ios, api, session])

  const switchMode = (next: EditorMode) => {
    if (!next || next === mode) return
    if (next === 'json') {
      setJsonText(formatServerJson(form))
      setMode('json')
    } else {
      const trimmed = jsonText.trim()
      if (trimmed && !gitUrl(trimmed)) {
        try {
          const parsed = mcpEntry(trimmed)
          const nextForm = entryToForm(parsed.name, parsed.entry)
          if (ios && nextForm.transport === 'stdio') nextForm.transport = 'http'
          setForm(nextForm)
        } catch {
          // Keep current form values if JSON isn't valid yet
        }
      }
      setMode('form')
    }
  }

  /** Parse the current Form or JSON input into either a git repo URL or `{ name, entry }`. */
  const resolveInput = (): { kind: 'repo'; url: string } | { kind: 'server'; name: string; entry: Record<string, unknown> } => {
    if (mode === 'form') {
      const { name, entry } = formToEntry(form)
      return { kind: 'server', name, entry }
    }
    const trimmed = jsonText.trim()
    if (!trimmed) throw new Error('Paste a server JSON object')
    if (gitUrl(trimmed)) {
      if (!canRepo) throw new Error('Git MCP repositories are not supported on this device; add a remote HTTP server instead')
      return { kind: 'repo', url: trimmed }
    }
    const { name, entry } = mcpEntry(trimmed)
    return { kind: 'server', name, entry: entry as Record<string, unknown> }
  }

  const jsonError = useMemo(() => {
    if (mode !== 'json') return null
    const trimmed = jsonText.trim()
    if (!trimmed) return null
    if (gitUrl(trimmed)) return canRepo ? null : 'Git repos are desktop-only; use a remote HTTP JSON entry'
    try {
      mcpEntry(trimmed)
      return null
    } catch (e) {
      return e instanceof Error ? e.message : String(e)
    }
  }, [mode, jsonText, canRepo])

  const canSubmit = useMemo(() => {
    if (mode === 'form') {
      if (!form.name.trim()) return false
      return form.transport === 'http' ? form.url.trim().length > 0 : form.command.trim().length > 0
    }
    return jsonText.trim().length > 0 && !jsonError
  }, [mode, form, jsonText, jsonError])

  const startOAuthFor = async (serverName: string) => {
    const local = useEngines.getState().active === LOCAL
    setProbe({ kind: 'signing_in', serverName, url: null, local })
    try {
      const url = await api.mcp_login(serverName)
      setProbe({ kind: 'signing_in', serverName, url, local })
      if (ios && local) void openAuthBrowser(url, false)
    } catch (e) {
      setProbe({ kind: 'failed', message: errorText(e) })
    }
  }

  const handleTest = async () => {
    if (!client) return
    let parsed: ReturnType<typeof resolveInput>
    try {
      parsed = resolveInput()
    } catch (e) {
      setProbe({ kind: 'failed', message: errorText(e) })
      return
    }
    if (parsed.kind === 'repo') {
      setProbe({ kind: 'failed', message: 'Git repositories are cloned on Add; test connection applies to server entries.' })
      return
    }
    setProbe({ kind: 'testing' })
    try {
      const res = await client.call<McpTestResult>('test_mcp_server', [parsed.name, parsed.entry])
      if (res.status === 'connected') {
        setProbe({ kind: 'connected', tools: res.tools, saved: false })
      } else if (res.status === 'auth_required') {
        setProbe({ kind: 'auth_required', serverName: parsed.name, saved: false })
      } else {
        setProbe({ kind: 'failed', message: res.error ?? 'Connection failed' })
      }
    } catch (e) {
      setProbe({ kind: 'failed', message: errorText(e) })
    }
  }

  const handleSave = async (thenOAuth = false) => {
    let parsed: ReturnType<typeof resolveInput>
    try {
      parsed = resolveInput()
    } catch (e) {
      setProbe({ kind: 'failed', message: errorText(e) })
      return
    }
    setSaving(true)
    try {
      if (parsed.kind === 'repo') {
        const r = await api.add_mcp_repo(parsed.url, null)
        invalidate()
        toast.success(`Cloned ${r.name}`)
        setOpen(false)
        resetAll()
        return
      }
      const server: McpServer = await api.add_mcp_server(parsed.name, parsed.entry)
      invalidate()
      if (server.auth === 'required' || thenOAuth) {
        setProbe({ kind: 'auth_required', serverName: server.name, saved: true })
        setSaving(false)
        if (thenOAuth) {
          await startOAuthFor(server.name)
        }
        return
      }
      if (server.connected) {
        toast.success(`Added ${server.name} (${server.tools.length} tools)`)
        setOpen(false)
        resetAll()
        return
      }
      setProbe({ kind: 'failed', message: server.reason ?? 'Server added, but did not connect yet' })
    } catch (e) {
      setProbe({ kind: 'failed', message: errorText(e) })
    } finally {
      setSaving(false)
    }
  }

  const updateKv = (field: 'headers' | 'env', id: number, part: 'key' | 'value', val: string) => {
    setForm((prev) => ({
      ...prev,
      [field]: prev[field].map((r) => (r.id === id ? { ...r, [part]: val } : r)),
    }))
    if (probe.kind !== 'idle') setProbe({ kind: 'idle' })
  }

  const addKv = (field: 'headers' | 'env') => {
    const id = nextRowId.current++
    setForm((prev) => ({ ...prev, [field]: [...prev[field], { id, key: '', value: '' }] }))
  }

  const removeKv = (field: 'headers' | 'env', id: number) => {
    setForm((prev) => {
      const next = prev[field].filter((r) => r.id !== id)
      return { ...prev, [field]: next.length > 0 ? next : [{ id: nextRowId.current++, key: '', value: '' }] }
    })
  }

  const patchForm = <K extends keyof McpFormState>(key: K, value: McpFormState[K]) => {
    setForm((prev) => ({ ...prev, [key]: value }))
    if (probe.kind !== 'idle') setProbe({ kind: 'idle' })
  }

  const busy = saving || probe.kind === 'testing' || probe.kind === 'signing_in'

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        setOpen(next)
        if (!next) resetAll()
      }}
    >
      <DialogTrigger asChild>
        <Button variant="ghost" size="sm">
          <Plus />
          Add
        </Button>
      </DialogTrigger>
      <DialogContent className="max-h-[85dvh] overflow-y-auto sm:max-w-[500px]">
        <DialogHeader>
          <div className="flex items-center justify-between gap-2 pr-6">
            <DialogTitle>Add MCP server</DialogTitle>
            <ToggleGroup
              type="single"
              variant="outline"
              size="sm"
              value={mode}
              onValueChange={(v) => v && switchMode(v as EditorMode)}
              aria-label="Editor mode"
            >
              <ToggleGroupItem value="form" className="h-7 px-2.5 text-xs">
                Form
              </ToggleGroupItem>
              <ToggleGroupItem value="json" className="h-7 px-2.5 text-xs">
                JSON
              </ToggleGroupItem>
            </ToggleGroup>
          </div>
          <DialogDescription>
            {ios
              ? 'Connect a remote HTTP MCP server with optional Bearer token, custom headers, or OAuth sign-in.'
              : 'Configure a remote HTTP or local stdio MCP server, test the connection, or paste JSON.'}
          </DialogDescription>
        </DialogHeader>

        {mode === 'form' ? (
          <div className="space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="mcp-name">Name</Label>
              <Input
                id="mcp-name"
                value={form.name}
                onChange={(e) => patchForm('name', e.target.value)}
                placeholder="linear"
                autoCapitalize="none"
                autoCorrect="off"
                spellCheck={false}
              />
            </div>

            {!ios && (
              <div className="space-y-1.5">
                <Label>Transport</Label>
                <ToggleGroup
                  type="single"
                  variant="outline"
                  size="sm"
                  value={form.transport}
                  onValueChange={(v) => v && patchForm('transport', v as TransportMode)}
                  className="justify-start"
                >
                  <ToggleGroupItem value="http" className="px-3 text-xs">
                    Remote HTTP
                  </ToggleGroupItem>
                  <ToggleGroupItem value="stdio" className="px-3 text-xs">
                    Local stdio
                  </ToggleGroupItem>
                </ToggleGroup>
              </div>
            )}

            {form.transport === 'http' ? (
              <>
                <div className="space-y-1.5">
                  <Label htmlFor="mcp-url">Server URL</Label>
                  <Input
                    id="mcp-url"
                    type="url"
                    value={form.url}
                    onChange={(e) => patchForm('url', e.target.value)}
                    placeholder="https://mcp.linear.app/mcp"
                    autoCapitalize="none"
                    autoCorrect="off"
                    spellCheck={false}
                  />
                </div>

                <div className="space-y-2">
                  <Label>Authentication</Label>
                  <ToggleGroup
                    type="single"
                    variant="outline"
                    size="sm"
                    value={form.authMode}
                    onValueChange={(v) => v && patchForm('authMode', v as AuthMode)}
                    className="justify-start"
                  >
                    <ToggleGroupItem value="oauth" className="px-2.5 text-xs">
                      OAuth / None
                    </ToggleGroupItem>
                    <ToggleGroupItem value="bearer" className="px-2.5 text-xs">
                      Bearer token
                    </ToggleGroupItem>
                    <ToggleGroupItem value="headers" className="px-2.5 text-xs">
                      Headers
                    </ToggleGroupItem>
                  </ToggleGroup>

                  {form.authMode === 'oauth' && (
                    <div className="space-y-2">
                      <p className="text-xs text-fg-secondary">
                        Connects directly, or prompts for OAuth sign-in automatically if the server requires authentication.
                      </p>
                      <Collapsible open={oauthOpen} onOpenChange={setOauthOpen}>
                        <CollapsibleTrigger asChild>
                          <button
                            type="button"
                            className="flex items-center gap-1 text-xs text-fg-secondary hover:text-fg-primary"
                          >
                            {oauthOpen ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" />}
                            Advanced OAuth options (optional)
                          </button>
                        </CollapsibleTrigger>
                        <CollapsibleContent className="mt-2 space-y-2 rounded-lg border border-border-subtle p-3">
                          <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
                            <div className="space-y-1">
                              <Label htmlFor="mcp-cid" className="text-xs">
                                Client ID
                              </Label>
                              <Input
                                id="mcp-cid"
                                value={form.oauthClientId}
                                onChange={(e) => patchForm('oauthClientId', e.target.value)}
                                placeholder="Optional"
                                className="h-8 text-xs"
                              />
                            </div>
                            <div className="space-y-1">
                              <Label htmlFor="mcp-csec" className="text-xs">
                                Client secret
                              </Label>
                              <Input
                                id="mcp-csec"
                                type="password"
                                value={form.oauthClientSecret}
                                onChange={(e) => patchForm('oauthClientSecret', e.target.value)}
                                placeholder="Vault placeholder or value"
                                className="h-8 text-xs"
                              />
                            </div>
                          </div>
                          <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
                            <div className="space-y-1">
                              <Label htmlFor="mcp-scopes" className="text-xs">
                                Scopes
                              </Label>
                              <Input
                                id="mcp-scopes"
                                value={form.oauthScopes}
                                onChange={(e) => patchForm('oauthScopes', e.target.value)}
                                placeholder="read write"
                                className="h-8 text-xs"
                              />
                            </div>
                            <div className="space-y-1">
                              <Label htmlFor="mcp-redir" className="text-xs">
                                Redirect URI
                              </Label>
                              <Input
                                id="mcp-redir"
                                value={form.oauthRedirectUri}
                                onChange={(e) => patchForm('oauthRedirectUri', e.target.value)}
                                placeholder="http://127.0.0.1:8765/callback"
                                className="h-8 text-xs"
                              />
                            </div>
                          </div>
                        </CollapsibleContent>
                      </Collapsible>
                    </div>
                  )}

                  {form.authMode === 'bearer' && (
                    <div className="space-y-1">
                      <Input
                        type="password"
                        value={form.bearerToken}
                        onChange={(e) => patchForm('bearerToken', e.target.value)}
                        placeholder="Paste Bearer token or vault placeholder"
                        autoCapitalize="none"
                        autoCorrect="off"
                        spellCheck={false}
                      />
                      <p className="text-xs text-fg-tertiary">
                        Sent as <code className="font-mono">Authorization: Bearer …</code>. Vault placeholders like{' '}
                        <code className="font-mono">{'{' + '{secret:NAME}}'}</code> are supported.
                      </p>
                    </div>
                  )}

                  {form.authMode === 'headers' && (
                    <div className="space-y-2">
                      {form.headers.map((row) => (
                        <div key={row.id} className="flex items-center gap-1.5">
                          <Input
                            value={row.key}
                            onChange={(e) => updateKv('headers', row.id, 'key', e.target.value)}
                            placeholder="Header name"
                            className="h-8 flex-1 font-mono text-xs"
                            autoCapitalize="none"
                            autoCorrect="off"
                            spellCheck={false}
                          />
                          <Input
                            value={row.value}
                            onChange={(e) => updateKv('headers', row.id, 'value', e.target.value)}
                            placeholder="Value or vault placeholder"
                            className="h-8 flex-1 font-mono text-xs"
                            autoCapitalize="none"
                            autoCorrect="off"
                            spellCheck={false}
                          />
                          <Button
                            type="button"
                            variant="ghost"
                            size="icon-sm"
                            aria-label="Remove header"
                            onClick={() => removeKv('headers', row.id)}
                          >
                            <Trash2 className="size-3.5" />
                          </Button>
                        </div>
                      ))}
                      <Button type="button" variant="ghost" size="sm" className="h-7 px-2 text-xs" onClick={() => addKv('headers')}>
                        <Plus className="size-3.5" />
                        Add header
                      </Button>
                    </div>
                  )}
                </div>
              </>
            ) : (
              <>
                <div className="space-y-1.5">
                  <Label htmlFor="mcp-cmd">Command</Label>
                  <Input
                    id="mcp-cmd"
                    value={form.command}
                    onChange={(e) => patchForm('command', e.target.value)}
                    placeholder="npx"
                    className="font-mono text-xs"
                  />
                </div>
                <div className="space-y-1.5">
                  <Label htmlFor="mcp-args">Arguments</Label>
                  <Input
                    id="mcp-args"
                    value={form.args}
                    onChange={(e) => patchForm('args', e.target.value)}
                    placeholder="-y @modelcontextprotocol/server-github"
                    className="font-mono text-xs"
                  />
                </div>
                <div className="space-y-1.5">
                  <Label htmlFor="mcp-cwd">Working directory (optional)</Label>
                  <Input
                    id="mcp-cwd"
                    value={form.cwd}
                    onChange={(e) => patchForm('cwd', e.target.value)}
                    placeholder="/path/to/repo"
                    className="font-mono text-xs"
                  />
                </div>
                <div className="space-y-2">
                  <Label>Environment variables</Label>
                  {form.env.map((row) => (
                    <div key={row.id} className="flex items-center gap-1.5">
                      <Input
                        value={row.key}
                        onChange={(e) => updateKv('env', row.id, 'key', e.target.value)}
                        placeholder="KEY"
                        className="h-8 flex-1 font-mono text-xs"
                      />
                      <Input
                        value={row.value}
                        onChange={(e) => updateKv('env', row.id, 'value', e.target.value)}
                        placeholder="Value or vault placeholder"
                        className="h-8 flex-1 font-mono text-xs"
                      />
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon-sm"
                        aria-label="Remove variable"
                        onClick={() => removeKv('env', row.id)}
                      >
                        <Trash2 className="size-3.5" />
                      </Button>
                    </div>
                  ))}
                  <Button type="button" variant="ghost" size="sm" className="h-7 px-2 text-xs" onClick={() => addKv('env')}>
                    <Plus className="size-3.5" />
                    Add variable
                  </Button>
                </div>
              </>
            )}
          </div>
        ) : (
          <div className="space-y-2">
            <Label htmlFor="mcp-json">
              {canRepo ? 'Server JSON or git URL' : 'Server JSON'}
            </Label>
            <Textarea
              id="mcp-json"
              value={jsonText}
              onChange={(e) => {
                setJsonText(e.target.value)
                if (probe.kind !== 'idle') setProbe({ kind: 'idle' })
              }}
              rows={9}
              spellCheck={false}
              className="font-mono text-xs"
              placeholder={
                ios
                  ? '{\n  "linear": {\n    "url": "https://mcp.linear.app/mcp"\n  }\n}'
                  : '{\n  "linear": {\n    "url": "https://mcp.linear.app/mcp"\n  }\n}\n\n// Or paste a git URL to clone a local MCP repo'
              }
            />
            {jsonError && <p className="text-xs text-destructive">{jsonError}</p>}
          </div>
        )}

        {/* Inline Test Connection & OAuth Sign-In status banner */}
        {probe.kind === 'testing' && (
          <div className="flex items-center gap-2 rounded-lg border border-border-subtle bg-muted/40 px-3 py-2.5 text-xs text-fg-secondary">
            <LoaderCircle className="size-4 animate-spin" />
            Testing connection…
          </div>
        )}

        {probe.kind === 'connected' && (
          <div className="space-y-2 rounded-lg border border-success-border/40 bg-success-bg/50 px-3 py-2.5 text-xs">
            <div className="flex items-center gap-2 font-medium text-fg-primary">
              <CheckCircle2 className="size-4 text-emerald-600 dark:text-emerald-400" />
              <span>
                {probe.saved ? 'Signed in & connected' : 'Connection succeeded'} · {probe.tools.length}{' '}
                {probe.tools.length === 1 ? 'tool' : 'tools'}
              </span>
            </div>
            {probe.tools.length > 0 && (
              <div className="flex flex-wrap gap-1">
                {probe.tools.slice(0, 8).map((t) => (
                  <Badge key={t} variant="secondary" className="font-mono text-[10px] font-normal">
                    {t}
                  </Badge>
                ))}
                {probe.tools.length > 8 && (
                  <Badge variant="outline" className="text-[10px] font-normal">
                    +{probe.tools.length - 8} more
                  </Badge>
                )}
              </div>
            )}
          </div>
        )}

        {probe.kind === 'auth_required' && (
          <div className="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-warning-border/50 bg-warning-bg/60 px-3 py-2.5 text-xs">
            <div className="flex items-center gap-2 text-fg-primary">
              <KeyRound className="size-4 shrink-0 text-amber-600 dark:text-amber-400" />
              <div>
                <p className="font-medium">OAuth sign-in required</p>
                <p className="text-fg-secondary">
                  {probe.saved
                    ? `Saved ${probe.serverName}. Sign in to authorize access.`
                    : 'Server is reachable and asks for OAuth authorization.'}
                </p>
              </div>
            </div>
            <Button
              type="button"
              size="sm"
              disabled={saving}
              onClick={() => (probe.saved ? void startOAuthFor(probe.serverName) : void handleSave(true))}
            >
              <KeyRound className="size-3.5" />
              Sign in with OAuth
            </Button>
          </div>
        )}

        {probe.kind === 'signing_in' && (
          <div className="space-y-2 rounded-lg border border-border-subtle bg-muted/40 px-3 py-2.5 text-xs">
            <div className="flex items-center justify-between gap-2">
              <span className="flex items-center gap-2 font-medium text-fg-primary">
                <LoaderCircle className="size-4 animate-spin" />
                Waiting for OAuth sign-in…
              </span>
              {probe.url && (
                <a
                  href={probe.url}
                  target="_blank"
                  rel="noreferrer"
                  className="inline-flex items-center gap-1 text-primary underline underline-offset-2"
                >
                  Open sign-in
                  <ExternalLink className="size-3" />
                </a>
              )}
            </div>
            <p className="text-fg-secondary">
              Complete authorization in the browser window; this dialog will update automatically once connected.
            </p>
          </div>
        )}

        {probe.kind === 'failed' && (
          <div className="flex items-start gap-2 rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2.5 text-xs text-destructive">
            <XCircle className="mt-0.5 size-4 shrink-0" />
            <span className="break-words">{probe.message}</span>
          </div>
        )}

        <DialogFooter className="flex-row flex-wrap items-center justify-between gap-2 sm:justify-between">
          <div className="flex items-center gap-2">
            {canTest && !(probe.kind === 'connected' && probe.saved) && (
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={!canSubmit || busy}
                onClick={() => void handleTest()}
              >
                <PlugZap className="size-3.5" />
                Test connection
              </Button>
            )}
          </div>
          <div className="flex items-center gap-2">
            {probe.kind === 'connected' && probe.saved ? (
              <Button
                type="button"
                size="sm"
                onClick={() => {
                  setOpen(false)
                  resetAll()
                }}
              >
                Done
              </Button>
            ) : (
              <>
                <Button type="button" variant="ghost" size="sm" onClick={() => setOpen(false)}>
                  Cancel
                </Button>
                {probe.kind === 'auth_required' ? (
                  <Button
                    type="button"
                    size="sm"
                    disabled={saving}
                    onClick={() => (probe.saved ? void startOAuthFor(probe.serverName) : void handleSave(true))}
                  >
                    {saving ? <LoaderCircle className="size-3.5 animate-spin" /> : <KeyRound className="size-3.5" />}
                    {probe.saved ? 'Sign in with OAuth' : 'Save & Sign in'}
                  </Button>
                ) : (
                  <Button
                    type="button"
                    size="sm"
                    disabled={!canSubmit || busy}
                    onClick={() => void handleSave(false)}
                  >
                    {saving && <LoaderCircle className="size-3.5 animate-spin" />}
                    Add server
                  </Button>
                )}
              </>
            )}
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
