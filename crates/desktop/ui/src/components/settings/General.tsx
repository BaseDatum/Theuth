import { useQuery, useQueryClient } from '@tanstack/react-query'
import { getVersion } from '@tauri-apps/api/app'
import { disable, enable, isEnabled } from '@tauri-apps/plugin-autostart'
import { useState } from 'react'
import { Row, Section, useAction } from '@/components/settings/shared'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { container } from '@/lib/platform'
import { queryKeys, useApi } from '@/lib/rpc'
import { isTauri } from '@/lib/tauri'
import { useUpdate } from '@/lib/update'
import { LOCAL, useHarnesses } from '@/link/harnesses'
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
  // Autostart and the window hotkey belong to the macOS shell with its own daemon (31 §9).
  const desktop = useHarnesses((s) => s.active) === LOCAL && container() === 'macos'
  const [theme, setThemeState] = useState(currentTheme)
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
  // 39 §4.3: the daemon's own updater; `theuth` on a terminal harness is not the bundled sidecar.
  const update = useQuery({
    queryKey: queryKeys.updateStatus,
    queryFn: () => api.update_status(),
    enabled: !!info,
  })

  const setAutostart = useAction((on: boolean) => (on ? enable() : disable()), {
    onSuccess: (_, on) => qc.setQueryData(AUTOSTART, on),
  })

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
            />
          </>
        )}
      </Section>
      {info && (
        <Section title="Harness">
          <Row label="Name" description={`theuth ${info.version}`}>
            <span className="text-sm">{info.name}</span>
          </Row>
          <Row label={`Data directory on ${info.name}`}>
            <code className="max-w-96 truncate font-mono text-[13px] text-fg-tertiary">
              {info.data_dir}
            </code>
          </Row>
          {update.data && (
            <Row
              label="Update"
              description={`${update.data.version} ${update.data.applied ? 'ready · restarts with the daemon' : 'available · `t --update` on the harness'}`}
            />
          )}
        </Section>
      )}
    </>
  )
}
