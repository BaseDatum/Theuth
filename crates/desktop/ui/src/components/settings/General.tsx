import { useQuery, useQueryClient } from '@tanstack/react-query'
import { disable, enable, isEnabled } from '@tauri-apps/plugin-autostart'
import { useState } from 'react'
import { Row, Section, refreshInfo, useAction } from '@/components/settings/shared'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { queryKeys, useApi } from '@/lib/rpc'
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

export function General() {
  const api = useApi()
  const qc = useQueryClient()
  const info = useConnection((s) => s.info)
  const [theme, setThemeState] = useState(currentTheme)
  const models = useQuery({ queryKey: queryKeys.models, queryFn: () => api.list_models() })
  const titleModel = useQuery({ queryKey: queryKeys.titleModel, queryFn: () => api.title_model() })
  const autostart = useQuery({ queryKey: AUTOSTART, queryFn: isEnabled })

  const setTitleModel = useAction((id: string) => api.set_title_model(id === 'off' ? null : id), {
    keys: [queryKeys.titleModel],
  })
  const setCompact = useAction((style: CompactStyle) => api.set_compact_style(style).then(() => refreshInfo(api)))
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
        <Row label="Default compact style" description="How a compaction builds the new context head.">
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
        <Row label="Open at login" description={autostart.isError ? 'Unavailable outside the app.' : undefined}>
          <Switch
            checked={autostart.data ?? false}
            disabled={!autostart.isSuccess || setAutostart.isPending}
            onCheckedChange={(v) => setAutostart.mutate(v)}
          />
        </Row>
        <Row label="Toggle window" description="Fixed in this version.">
          <kbd className="rounded-md border px-2 py-0.5 font-sans text-sm">⌥Space</kbd>
        </Row>
      </Section>
      {info && (
        <p className="text-[13px] text-fg-tertiary">
          Daemon {info.version} · {info.data_dir}
        </p>
      )}
    </>
  )
}
