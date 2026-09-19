import { useQuery, useQueryClient } from '@tanstack/react-query'
import { getVersion } from '@tauri-apps/api/app'
import { disable, enable, isEnabled } from '@tauri-apps/plugin-autostart'
import { useState } from 'react'
import { Row, Section, refreshInfo, useAction } from '@/components/settings/shared'
import { Input } from '@/components/ui/input'
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
import type { CompactStyle } from '@/rpc/bindings/CompactStyle'
import { useConnection } from '@/store/connection'

const THEMES: [Theme, string][] = [
  ['system', 'System'],
  ['light', 'Light'],
  ['dark', 'Dark'],
]

const COMPACT: [CompactStyle, string][] = [
  ['summary', 'Summary'],
  ['cut', 'Cut'],
  ['snap', 'Snap'],
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
  const models = useQuery({
    queryKey: queryKeys.models,
    queryFn: () => api.list_models(),
  })
  const titleModel = useQuery({
    queryKey: queryKeys.titleModel,
    queryFn: () => api.title_model(),
  })
  const visionModel = useQuery({
    queryKey: queryKeys.visionModel,
    queryFn: () => api.vision_model(),
  })
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
  // 39 §4.3: the daemon's own updater; `wth` on a terminal harness is not the bundled sidecar.
  const update = useQuery({
    queryKey: queryKeys.updateStatus,
    queryFn: () => api.update_status(),
    enabled: !!info,
  })

  const setTitleModel = useAction((id: string) => api.set_title_model(id === 'off' ? null : id), {
    keys: [queryKeys.titleModel],
  })
  const setVisionModel = useAction((id: string) => api.set_vision_model(id === 'off' ? null : id), {
    keys: [queryKeys.visionModel],
  })
  const setCompact = useAction((style: CompactStyle) =>
    api.set_compact_style(style).then(() => refreshInfo(api)),
  )
  const setSubagentTimeout = useAction((seconds: number) =>
    api.set_subagent_timeout(seconds).then(() => refreshInfo(api)),
  )
  const setMaxToolRounds = useAction((rounds: number) =>
    api.set_max_tool_rounds(rounds).then(() => refreshInfo(api)),
  )
  const setAutostart = useAction((on: boolean) => (on ? enable() : disable()), {
    onSuccess: (_, on) => qc.setQueryData(AUTOSTART, on),
  })

  // 52 §3.2: only models whose backend can actually receive images (`ModelInfo.image` is effective support).
  const visionModels = models.data?.filter((m) => m.image) ?? []
  const vision = visionModel.data
  // A saved selection whose provider went away stays visible as unavailable, never as Off.
  const unavailable =
    vision && models.isSuccess && !visionModels.some((m) => m.id === vision.id) ? vision.id : null

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
        <Row label="Title model" description="Names new chats after the first exchange.">
          <Select
            value={titleModel.data ? titleModel.data.id : 'off'}
            disabled={titleModel.isPending || setTitleModel.isPending}
            onValueChange={(v) => setTitleModel.mutate(v)}
          >
            <SelectTrigger className="max-w-64">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="off">Off</SelectItem>
              {models.data?.map((m) => (
                <SelectItem key={m.id} value={m.id}>
                  {m.id}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Row>
        <Row
          label="Vision fallback model"
          description={
            visionModel.isError
              ? `Could not read the vision fallback: ${visionModel.error.message}`
              : unavailable
                ? `${unavailable} is selected but not available on this harness; pick another or Off.`
                : 'Used only when the main model cannot view images. It receives the image and a question written by the main model. Images may go to a different configured provider, and each new image batch costs a question call and a vision call; text redaction cannot remove secrets shown in pixels.'
          }
        >
          <Select
            value={vision ? vision.id : 'off'}
            disabled={!visionModel.isSuccess || setVisionModel.isPending}
            onValueChange={(v) => setVisionModel.mutate(v)}
          >
            <SelectTrigger aria-label="Vision fallback model" className="max-w-64">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="off">Off</SelectItem>
              {visionModels.map((m) => (
                <SelectItem key={m.id} value={m.id}>
                  {m.id}
                </SelectItem>
              ))}
              {unavailable && <SelectItem value={unavailable}>{unavailable} (unavailable)</SelectItem>}
            </SelectContent>
          </Select>
        </Row>
        <Row
          label="Default compact style"
          description="How a compaction builds the new context head."
        >
          <Select
            value={info?.compact_style}
            disabled={!info || setCompact.isPending}
            onValueChange={(v) => setCompact.mutate(v as CompactStyle)}
          >
            <SelectTrigger className="w-40">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {COMPACT.map(([id, label]) => (
                <SelectItem key={id} value={id}>
                  {label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Row>
        <Row
          label="Subagent timeout"
          description="Maximum runtime for a newly spawned subagent turn, in seconds."
        >
          <Input
            key={info?.subagent_timeout_secs}
            type="number"
            defaultValue={info?.subagent_timeout_secs}
            disabled={!info || setSubagentTimeout.isPending}
            className="w-28 text-right"
            onBlur={(e) => {
              const seconds = Number(e.currentTarget.value)
              if (seconds !== info?.subagent_timeout_secs) setSubagentTimeout.mutate(seconds)
            }}
            onKeyDown={(e) => {
              if (e.key === 'Enter') e.currentTarget.blur()
            }}
          />
        </Row>
        <Row
          label="Tool rounds per turn"
          description="Tool-call rounds a turn may run before the model is asked to reply with text. New sessions only."
        >
          <Input
            key={info?.max_tool_rounds}
            type="number"
            min={1}
            defaultValue={info?.max_tool_rounds}
            disabled={!info || setMaxToolRounds.isPending}
            className="w-28 text-right"
            onBlur={(e) => {
              const rounds = Math.floor(Number(e.currentTarget.value))
              if (rounds >= 1 && rounds !== info?.max_tool_rounds) setMaxToolRounds.mutate(rounds)
            }}
            onKeyDown={(e) => {
              if (e.key === 'Enter') e.currentTarget.blur()
            }}
          />
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
                  ? `wth ${appVersion.data ?? ''} · ${pending} installs when you quit`
                  : `wth ${appVersion.data ?? ''}`
              }
            />
          </>
        )}
      </Section>
      {info && (
        <Section title="Harness">
          <Row label="Name" description={`wth ${info.version}`}>
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
              description={`${update.data.version} ${update.data.applied ? 'ready · restarts with the daemon' : 'available · `wth --update` on the harness'}`}
            />
          )}
        </Section>
      )}
    </>
  )
}
