import { useQuery, useQueryClient } from '@tanstack/react-query'
import { getVersion } from '@tauri-apps/api/app'
import { disable, enable, isEnabled } from '@tauri-apps/plugin-autostart'
import { LoaderCircle } from 'lucide-react'
import { useState } from 'react'
import { toast } from 'sonner'
import { Row, Section, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { useBoards } from '@/lib/boards'
import { isDesktop } from '@/lib/platform'
import { queryKeys, useApi } from '@/lib/rpc'
import { checkUpdate, installUpdate, isTauri } from '@/lib/tauri'
import { useUpdate } from '@/lib/update'
import { INITIAL, diagnostics } from '@/lib/link-status'
import { LOCAL, useEngines } from '@/link/engines'
import { type Theme, currentTheme, setTheme } from '@/lib/theme'
import { useConnection } from '@/store/connection'

const THEMES: [Theme, string][] = [
  ['system', 'System'],
  ['light', 'Light'],
  ['dark', 'Dark'],
]

const AUTOSTART = ['autostart'] as const
const APP_VERSION = ['app-version'] as const

export function General() {
  const api = useApi()
  const qc = useQueryClient()
  const info = useConnection((s) => s.info)
  // Autostart and the window hotkey belong to the desktop shell with its own daemon (31 §9).
  const desktop = useEngines((s) => s.active) === LOCAL && isDesktop()
  const [theme, setThemeState] = useState(currentTheme)
  const serif = useBoards((s) => s.serif)
  const autostart = useQuery({
    queryKey: AUTOSTART,
    queryFn: isEnabled,
    enabled: desktop,
  })
  // 39 §4.2: the bundle's version and the one that installs at quit.
  const pending = useUpdate((s) => s.pending)
  const appVersion = useQuery({
    queryKey: APP_VERSION,
    queryFn: getVersion,
    enabled: desktop && isTauri(),
  })
  // 39 §4.3: the daemon's own updater; `theuth` on a terminal engine is not the bundled sidecar.
  const update = useQuery({
    queryKey: queryKeys.updateStatus,
    queryFn: () => api.update_status(),
    enabled: !!info,
  })
  // 59 §3.2: a missing documents helper silently disables notes, knowledge editing and meeting notes — say so here.
  const documents = useQuery({ queryKey: ['documents-status'], queryFn: () => api.documents_status(), retry: false })

  const setAutostart = useAction((on: boolean) => (on ? enable() : disable()), {
    onSuccess: (_, on) => qc.setQueryData(AUTOSTART, on),
  })
  const check = useAction(checkUpdate, {
    onSuccess: (version) => {
      useUpdate.setState({ pending: version })
      if (!version) toast.message('Up to date')
    },
  })
  const install = useAction(installUpdate)
  // `install_update` answers before the quit handshake runs; `isSuccess` holds until the process is gone.
  const restarting = install.isPending || install.isSuccess

  return (
    <>
      <Section title="General">
        <Row label="Theme">
          <Select
            value={theme}
            onValueChange={(v) => {
              setTheme(v as Theme)
              setThemeState(v as Theme)
            }}
          >
            <SelectTrigger className="w-40">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {THEMES.map(([id, label]) => (
                <SelectItem key={id} value={id}>
                  {label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Row>
        <Row label="Serif prose on boards" description="Board prose in Newsreader; off uses the interface face. Paths, captions and gutters stay mono.">
          <Switch checked={serif} onCheckedChange={useBoards.getState().setSerif} />
        </Row>
        {desktop && (
          <>
            <Row
              label="Open at login"
              description={autostart.isError ? 'Unavailable outside the app.' : undefined}
            >
              <Switch
                checked={autostart.data ?? false}
                disabled={!autostart.isSuccess || setAutostart.isPending}
                onCheckedChange={(v) => setAutostart.mutate(v)}
              />
            </Row>
            <Row label="Toggle window" description="Fixed in this version.">
              <kbd className="rounded-md border px-2 py-0.5 font-sans text-sm">⌥Space</kbd>
            </Row>
            <Row
              label="App"
              description={
                pending
                  ? `theuth ${appVersion.data ?? ''} · ${pending} installs when you quit`
                  : `theuth ${appVersion.data ?? ''}`
              }
            >
              {pending ? (
                <Button variant="outline" size="sm" disabled={restarting} onClick={() => install.mutate(undefined)}>
                  {restarting && <LoaderCircle className="animate-spin" />} {restarting ? 'Restarting…' : 'Install and restart'}
                </Button>
              ) : (
                <Button variant="outline" size="sm" disabled={check.isPending} onClick={() => check.mutate(undefined)}>
                  {check.isPending ? 'Checking…' : 'Check for updates'}
                </Button>
              )}
            </Row>
          </>
        )}
      </Section>
      {info && (
        <Section title="Node">
          <Row label="Name" description={`theuth ${info.version}`}>
            <span className="text-sm">{info.name}</span>
          </Row>
          <Row label={`Data directory on ${info.name}`}>
            <code className="max-w-96 truncate font-mono text-[13px] text-fg-tertiary">
              {info.data_dir}
            </code>
          </Row>
          {documents.data && (
            <Row
              label="Documents"
              description={
                documents.data.found
                  ? `Helper: theuth-documents ${documents.data.version}`
                  : `Helper missing at ${documents.data.path} — notes, knowledge editing and meeting notes are disabled. Reinstall the app (or run just install).`
              }
            >
              <span className={documents.data.found ? 'text-sm text-fg-tertiary' : 'text-sm text-danger'}>{documents.data.found ? 'ok' : 'missing'}</span>
            </Row>
          )}
          {update.data && (
            <Row
              label="Update"
              description={`${update.data.version} ${update.data.applied ? 'ready · restarts with the daemon' : 'available · `t --update` on the node'}`}
            />
          )}
        </Section>
      )}
      <LinkDiagnostics />
    </>
  )
}

/** Settings › General › Connection (LINK-MSG): the active link engine's socket as text, with "Copy diagnostics". */
function LinkDiagnostics() {
  const engine = useEngines((s) => s.engines.find((h) => h.id === s.active))
  const link = useEngines((s) => s.link[s.active]) ?? INITIAL
  if (!engine) return null
  const text = diagnostics(link, Date.now(), engine)
  return (
    <Section
      title="Connection"
      actions={
        <Button variant="outline" size="sm" onClick={() => navigator.clipboard.writeText(text).then(() => toast.message('Copied'), () => toast.error('Copy failed'))}>
          Copy diagnostics
        </Button>
      }
    >
      <pre className="font-mono text-[13px] whitespace-pre-wrap break-all text-fg-secondary">{text}</pre>
    </Section>
  )
}
