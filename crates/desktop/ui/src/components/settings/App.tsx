import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { getVersion } from '@tauri-apps/api/app'
import { disable, enable, isEnabled } from '@tauri-apps/plugin-autostart'
import { LoaderCircle } from 'lucide-react'
import { toast } from 'sonner'
import { Row, Section, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { isDesktop } from '@/lib/platform'
import { queryKeys, useApi } from '@/lib/rpc'
import { checkUpdate, installUpdate, isTauri } from '@/lib/tauri'
import { useUpdate } from '@/lib/update'
import { INITIAL, diagnostics } from '@/lib/link-status'
import { LOCAL, useEngines } from '@/link/engines'
import { useConnection } from '@/store/connection'
import { useOnboardingHere } from '@/components/onboarding/OnboardingHost'
import { useOnboarding } from '@/lib/onboarding/state'

const AUTOSTART = ['autostart'] as const
const APP_VERSION = ['app-version'] as const

/** 188 A2: Settings › General › App — the desktop app, the node it talks to and the link to it. */
export function App() {
  const canQuery1 = useQuerySupport(["update_status"])
  const canQuery1_2 = useQuerySupport(["documents_status"])
  const api = useApi()
  const qc = useQueryClient()
  const info = useConnection((s) => s.info)
  const onboarding = useOnboardingHere()
  // Autostart and app updates belong to the desktop shell with its own daemon (31 §9).
  const desktop = useEngines((s) => s.active) === LOCAL && isDesktop()
  const autostart = useQuery({
    queryKey: AUTOSTART,
    queryFn: isEnabled,
    enabled: canQuery1 && (desktop),
  })
  // 39 §4.2: the bundle's version and the one that installs at quit.
  const pending = useUpdate((s) => s.pending)
  const appVersion = useQuery({
    queryKey: APP_VERSION,
    queryFn: getVersion,
    enabled: canQuery1 && (desktop && isTauri()),
  })
  // 39 §4.3: the daemon's own updater; `theuth` on a terminal engine is not the bundled sidecar.
  const update = useQuery({
    queryKey: queryKeys.updateStatus,
    queryFn: () => api.update_status(),
    enabled: canQuery1 && (!!info),
  })
  // 59 §3.2: a missing documents helper silently disables notes, knowledge editing and meeting notes — say so here.
  const documents = useQuery({ enabled: canQuery1_2, queryKey: ['documents-status'], queryFn: () => api.documents_status(), retry: false })

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
      {desktop && (
        <Section title="Desktop app">
          <Row label="Open at login" description={autostart.isError ? 'Unavailable outside the app.' : undefined}>
            <Switch checked={autostart.data ?? false} disabled={!autostart.isSuccess || setAutostart.isPending} onCheckedChange={(v) => setAutostart.mutate(v)} />
          </Row>
          <Row label="App" description={pending ? `theuth ${appVersion.data ?? ''} · ${pending} installs when you quit` : `theuth ${appVersion.data ?? ''}`}>
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
        </Section>
      )}
      {onboarding && (
        // 215: back to Welcome with the checklist, visits and tours reset; finishing or skipping sets the flag again.
        <Section title="Onboarding">
          <Row label="Welcome tour" description="Connect a model, pick a default and start a first chat again, then take the tour.">
            <Button
              variant="outline"
              size="sm"
              onClick={() => {
                useOnboarding.getState().reset()
                useOnboarding.getState().openWizard('welcome')
              }}
            >
              Restart the welcome tour
            </Button>
          </Row>
        </Section>
      )}
      {info && (
        // The removed Name row carried the node's version; it stays beside the title (188 A2).
        <Section title="Node" actions={<span className="text-[13px] text-fg-secondary">{`theuth ${info.version}`}</span>}>
          <Row label={`Data directory on ${info.name}`}>
            <code title={info.data_dir} className="max-w-96 truncate font-mono text-[13px] text-fg-tertiary">
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

/** Settings › General › App › Connection (LINK-MSG): the active link engine's socket as text, with "Copy diagnostics". */
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
