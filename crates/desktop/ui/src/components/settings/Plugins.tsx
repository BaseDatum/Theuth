// 216 §9.6, contract §12.6: Settings › Plugins — every external plugin with its state, the enable switch, Restart, its
// log, Uninstall, its order in the sidebar and status strip; expanded, the card waiting for a press (Approve), what the
// verified grant holds (read-only in v1: narrowing is editing the manifest), per-placement Hide, tools, stderr tail.
// `?focus=<name>` (the install card's Review) expands and scrolls to that row.
import { useQuery } from '@tanstack/react-query'
import { openPath } from '@tauri-apps/plugin-opener'
import { ArrowDown, ArrowUp, ChevronRight, FolderOpen, RefreshCw, RotateCw, ScrollText, Trash2 } from 'lucide-react'
import { type ReactNode, useEffect, useRef, useState } from 'react'
import { GrantLines, RISK_WORD, grantedLines, shown } from '@/components/plugins/GrantLines'
import { PluginIcon } from '@/components/plugins/icons'
import { LogsSheet } from '@/components/plugins/LogsSheet'
import { NewPluginPopover } from '@/components/plugins/NewPluginPopover'
import { PluginDot } from '@/components/plugins/PluginDot'
import { STATE_DOT, STATE_LABEL } from '@/components/plugins/state'
import { type Action, Empty, Section, errorText, useAction } from '@/components/settings/shared'
import type { TabProps } from '@/components/settings/types'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from '@/components/ui/alert-dialog'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Switch } from '@/components/ui/switch'
import { notify as toast } from '@/lib/notify'
import { isDesktop } from '@/lib/platform'
import { queryKeys, useApi } from '@/lib/rpc'
import { useQuerySupport } from '@/rpc/capabilities'
import type { CardKind, InstallCard, PlacementInfo, PluginInfo, PluginRunState } from '@/rpc/bindings/method.plugins.output'

/** The mutations every row shares; owned by `Plugins`. Each refetches `plugin_list` and every `plugin_get` on success. */
type Actions = {
  enable: Action<{ name: string; on: boolean }, PluginInfo>
  restart: Action<string, PluginInfo>
  hide: Action<{ name: string; placement: string; hidden: boolean }, PluginInfo>
  order: Action<string[], PluginInfo[]>
  uninstall: Action<{ name: string; keepData: boolean }, unknown>
  approve: Action<{ name: string; card: InstallCard }, PluginInfo>
}

function usePluginActions(): Actions {
  const api = useApi()
  const keys = [queryKeys.plugins]
  return {
    enable: useAction((v: { name: string; on: boolean }) => api.plugin_set_enabled(v.name, v.on), { keys }),
    restart: useAction((name: string) => api.plugin_restart(name), { keys }),
    hide: useAction((v: { name: string; placement: string; hidden: boolean }) => api.plugin_set_hidden(v.name, v.placement, v.hidden), { keys }),
    order: useAction((order: string[]) => api.plugin_set_order(order), { keys }),
    uninstall: useAction((v: { name: string; keepData: boolean }) => api.plugin_uninstall(v.name, v.keepData), {
      keys,
      onSuccess: (_, v) => toast.success(`Uninstalled ${v.name}`, { description: 'Its folder is in plugins/.trash.' }),
    }),
    // Exactly the card's hashes: a card that changed since this page read it answers "review it again" (`Stale`).
    approve: useAction((v: { name: string; card: InstallCard }) => api.plugin_approve(v.name, v.card.manifest_sha256, v.card.code_sha256), {
      keys,
      onSuccess: (p) => toast.success(`Approved ${p.name}`),
    }),
  }
}

/** `names` with the one at `i` swapped with its neighbour `by` away (-1 up, 1 down). */
export function moved(names: readonly string[], i: number, by: -1 | 1): string[] {
  const next = [...names]
  const j = i + by
  if (j < 0 || j >= next.length) return next
  ;[next[i], next[j]] = [next[j] as string, next[i] as string]
  return next
}

export function Plugins({ focus }: TabProps) {
  const can = useQuerySupport(['plugin_list'])
  const api = useApi()
  const list = useQuery({ enabled: can, queryKey: queryKeys.plugins, queryFn: () => api.plugin_list() })
  const act = usePluginActions()
  const names = list.data?.map((p) => p.name) ?? []
  return (
    <Section
      title="Plugins"
      actions={
        <>
          <Button variant="ghost" size="sm" disabled={list.isFetching} onClick={() => void list.refetch()}>
            <RefreshCw className={list.isFetching ? 'animate-spin' : undefined} />
            Refresh
          </Button>
          <NewPluginPopover />
        </>
      }
    >
      {list.error && <p className="py-2 text-[13px] break-words text-destructive">{errorText(list.error)}</p>}
      {list.data?.length === 0 && (
        <Empty>
          No plugins yet. A plugin is a folder under plugins/ with a plugin.yaml and a program in any language; it can add
          sidebar icons, status items, tiles and commands. New plugin scaffolds one; nothing runs until you approve it.
        </Empty>
      )}
      {list.data?.map((p, i) => (
        <PluginRow key={p.name} info={p} names={names} index={i} focused={focus === p.name} act={act} />
      ))}
    </Section>
  )
}

/** The state chip: its dot and word (`STATE_LABEL`). */
function StateChip({ state }: { state: PluginRunState }) {
  return (
    <span className="flex shrink-0 items-center gap-1.5 text-[13px] text-fg-secondary">
      <PluginDot dot={STATE_DOT[state]} className="size-2" />
      {STATE_LABEL[state]}
    </span>
  )
}

function PluginRow({ info: p, names, index, focused, act }: { info: PluginInfo; names: readonly string[]; index: number; focused: boolean; act: Actions }) {
  // A card waiting for a press opens the row, so its Approve is in sight.
  const [open, setOpen] = useState(focused || p.needs_approval)
  const [logs, setLogs] = useState(false)
  const row = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (!focused) return
    setOpen(true)
    row.current?.scrollIntoView({ block: 'start', behavior: 'smooth' })
  }, [focused])
  const icon = p.placements[0]?.icon ?? 'puzzle'
  return (
    <Collapsible ref={row} open={open} onOpenChange={setOpen} data-plugin={p.name} className="scroll-mt-4 border-b border-border-subtle py-3 last:border-0">
      <div className="flex items-center gap-3">
        <CollapsibleTrigger asChild>
          <button type="button" className="flex min-w-0 flex-1 flex-wrap items-center gap-x-2 gap-y-1 text-left sm:flex-nowrap">
            <ChevronRight className={`size-4 shrink-0 text-fg-tertiary transition-transform ${open ? 'rotate-90' : ''}`} />
            <PluginIcon icon={icon} className="size-4 shrink-0 text-fg-secondary" />
            <span className="text-sm font-medium break-all">{p.name}</span>
            {p.version && <span className="shrink-0 text-[13px] text-fg-tertiary">{shown(p.version)}</span>}
            {p.needs_approval && p.state !== 'awaiting_approval' && (
              <Badge variant="outline" className="shrink-0 font-normal">
                approval waiting
              </Badge>
            )}
          </button>
        </CollapsibleTrigger>
        <StateChip state={p.state} />
        <Switch checked={p.enabled} disabled={act.enable.isPending} aria-label={`Enable ${p.name}`} onCheckedChange={(on) => act.enable.mutate({ name: p.name, on })} />
        <Uninstall name={p.name} act={act} />
      </div>
      {p.description && <p className="mt-1 ml-6 text-[13px] break-words text-fg-secondary">{shown(p.description)}</p>}
      {p.error && <p className="mt-1 ml-6 text-[13px] break-words text-destructive">{shown(p.error)}</p>}
      <div className="mt-2 ml-6 flex flex-wrap items-center gap-2">
        <Button variant="outline" size="sm" disabled={act.restart.isPending} onClick={() => act.restart.mutate(p.name)}>
          <RotateCw />
          Restart
        </Button>
        <Button variant="outline" size="sm" onClick={() => setLogs(true)}>
          <ScrollText />
          Logs
        </Button>
        {isDesktop() && (
          <Button variant="outline" size="sm" onClick={() => void openPath(p.dir).catch((e: unknown) => toast.error(errorText(e)))}>
            <FolderOpen />
            Open folder
          </Button>
        )}
        <span className="ml-auto flex items-center gap-1">
          <Button variant="ghost" size="icon-sm" aria-label={`Move ${p.name} up`} disabled={index === 0 || act.order.isPending} onClick={() => act.order.mutate(moved(names, index, -1))}>
            <ArrowUp />
          </Button>
          <Button variant="ghost" size="icon-sm" aria-label={`Move ${p.name} down`} disabled={index === names.length - 1 || act.order.isPending} onClick={() => act.order.mutate(moved(names, index, 1))}>
            <ArrowDown />
          </Button>
        </span>
      </div>
      <LogsSheet name={p.name} open={logs} onOpenChange={setLogs} />
      <CollapsibleContent className="mt-3 ml-6">
        <Detail info={p} act={act} />
      </CollapsibleContent>
    </Collapsible>
  )
}

/** Uninstall behind a confirmation; Keep data leaves `plugin-data/<name>` where it is. */
function Uninstall({ name, act }: { name: string; act: Actions }) {
  const [keepData, setKeepData] = useState(false)
  return (
    <AlertDialog onOpenChange={(open) => open && setKeepData(false)}>
      <AlertDialogTrigger asChild>
        <Button variant="ghost" size="icon-sm" aria-label={`Uninstall ${name}`}>
          <Trash2 />
        </Button>
      </AlertDialogTrigger>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Uninstall {name}?</AlertDialogTitle>
          <AlertDialogDescription>It stops, its approval is dropped, and its folder moves to plugins/.trash. Installing it again needs a new approval.</AlertDialogDescription>
        </AlertDialogHeader>
        <label className="flex items-center justify-between gap-3 text-sm">
          <span>
            Keep its data
            <span className="block text-[13px] text-fg-secondary">plugin-data/{name} stays where it is</span>
          </span>
          <Switch checked={keepData} onCheckedChange={setKeepData} aria-label="Keep its data" />
        </label>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction onClick={() => act.uninstall.mutate({ name, keepData })}>Uninstall</AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}

/** What a waiting card is about. */
export const CARD_WORDS: Record<CardKind, string> = {
  install: 'Waiting for your approval to run',
  widen: 'Asks for more than you approved',
  code_changed: 'Its code changed since you approved it',
}

const PLACEMENT_KIND: Record<PlacementInfo['kind'], string> = { sidebar: 'sidebar icon', status: 'status bar', tile: 'tile', command: 'command' }

function Block({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1">
      <h4 className="text-xs font-semibold tracking-wide text-fg-tertiary uppercase">{title}</h4>
      {children}
    </div>
  )
}

/** The expanded row: `plugin_get(name)`, fetched while it is open. */
function Detail({ info: p, act }: { info: PluginInfo; act: Actions }) {
  const api = useApi()
  const d = useQuery({ queryKey: queryKeys.plugin(p.name), queryFn: () => api.plugin_get(p.name) })
  if (d.error) return <p className="pb-2 text-[13px] break-words text-destructive">{errorText(d.error)}</p>
  if (!d.data) return <p className="pb-2 text-[13px] text-fg-tertiary">Loading…</p>
  const { card, granted, requested, stderr_tail: stderr, warnings } = d.data
  return (
    <div className="flex flex-col gap-5 pb-2">
      {p.needs_approval && card && (
        <div className="flex flex-col gap-2 rounded-lg border border-border-subtle bg-layer-1 p-3">
          <p className="text-sm font-medium">{CARD_WORDS[card.kind]}</p>
          {card.unverified && <p className="text-[13px] text-warning">The saved approval for {p.name} could not be verified; approve it again to run it.</p>}
          <GrantLines lines={card.lines} />
          {card.changed_paths.length > 0 && (
            <p className="text-[13px] text-fg-secondary">
              Changed: <span className="font-mono text-xs break-all">{card.changed_paths.join(', ')}</span>
            </p>
          )}
          <div className="flex justify-end">
            <Button size="sm" disabled={act.approve.isPending} onClick={() => act.approve.mutate({ name: p.name, card })}>
              Approve
            </Button>
          </div>
        </div>
      )}
      <Block title="Granted">
        {granted ? <GrantLines lines={grantedLines(granted)} /> : <p className="text-[13px] text-fg-tertiary">Nothing yet: it runs once you approve it.</p>}
        {granted && <p className="text-[13px] text-fg-tertiary">To grant less, edit its plugin.yaml.</p>}
      </Block>
      {p.placements.length > 0 && (
        <Block title="Placements">
          {p.placements.map((pl) => (
            <div key={pl.id} className="flex items-center gap-3 border-b border-border-subtle py-2 last:border-0">
              <PluginIcon icon={pl.icon} className="size-4 shrink-0 text-fg-secondary" />
              <span className="min-w-0 flex-1 text-sm break-words">
                {shown(pl.label)}
                <span className="ml-2 text-[13px] text-fg-tertiary">{pl.command ? pl.command.slash : PLACEMENT_KIND[pl.kind]}</span>
              </span>
              <Switch
                checked={!pl.hidden}
                disabled={act.hide.isPending}
                aria-label={`Show ${pl.label}`}
                title={pl.kind === 'command' ? 'Hidden commands still run when typed' : undefined}
                onCheckedChange={(shown) => act.hide.mutate({ name: p.name, placement: pl.id, hidden: !shown })}
              />
            </div>
          ))}
        </Block>
      )}
      {requested.tools.length > 0 && (
        <Block title="Tools for the model">
          {requested.tools.map((t) => (
            <div key={t.name} className="flex items-baseline gap-3 border-b border-border-subtle py-2 text-sm last:border-0">
              <span className="min-w-0 flex-1 font-mono text-xs break-all">{t.name}</span>
              <span className="shrink-0 text-[13px] text-fg-tertiary">{RISK_WORD[t.risk]}</span>
            </div>
          ))}
        </Block>
      )}
      <Block title="Folder">
        <code className="font-mono text-xs break-all text-fg-secondary select-all">{shown(p.dir)}</code>
      </Block>
      {warnings.length > 0 && (
        <Block title="Warnings">
          {warnings.map((w) => (
            <p key={w} className="text-[13px] break-words text-warning">
              {shown(w)}
            </p>
          ))}
        </Block>
      )}
      {stderr.length > 0 && (
        <Block title="Last stderr lines">
          <pre className="max-h-60 overflow-auto rounded-md bg-layer-2 px-3 py-2 font-mono text-xs leading-relaxed whitespace-pre-wrap break-all text-fg-secondary">{stderr.join('\n')}</pre>
        </Block>
      )}
    </div>
  )
}
