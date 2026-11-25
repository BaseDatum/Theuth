// 216 §9, contract §12.2: the client's view of external-plugin placements, one store per engine. Seeded from
// `plugin_list` + `plugin_surfaces` (snapshot first), then kept current by the five `plugins.ui` variants, whose
// listeners live here (not in `lib/rpc.ts`). Surfaces are fetched per placement with `plugin_surface` when shown.
import { useEffect, useMemo, useState } from 'react'
import { type StoreApi, createStore, useStore } from 'zustand'
import { supports } from '@/rpc/discovery'
import type { PluginUi } from '@/rpc/bindings/event.plugins_2eui.output'
import type { Badge, CommandInfo, CommandUi, Dot, PlacementInfo, PlacementKind, PluginInfo, SurfaceContent, SurfaceSnapshot } from '@/rpc/bindings/method.plugins.output'
import { useConnection } from '@/store/connection'
import { type Stores, active, useScope } from '@/store/engine-scope'
import { on } from '@/store/sessions'

/** A placement's key in the store's maps. */
export const placementKey = (plugin: string, placement: string): string => `${plugin}/${placement}`

/** An ephemeral command reply card (`ui` of `plugin_command`), shown above the composer of `session` (a new chat's:
 * `null`) that ran it, until closed. */
export type CommandCard = { plugin: string; slash: string; session: string | null; ui: CommandUi }

export type PluginsState = {
  /** `plugin_list`; `null` until the first load. */
  plugins: readonly PluginInfo[] | null
  /** Latest dot/badge/label/tooltip per placement (`plugin_surfaces`, then `PluginStatus`). */
  status: Readonly<Record<string, SurfaceSnapshot>>
  /** Newest content revision announced per placement (`PluginSurface`); a shown surface refetches past its own. */
  revision: Readonly<Record<string, number>>
  /** `PluginInvalidate` count per placement: a shown OpenUI surface re-runs its queries when it moves. */
  invalidated: Readonly<Record<string, number>>
  /** Fetched content per placement (`plugin_surface`). */
  contents: Readonly<Record<string, SurfaceContent>>
  /** Bumped by `PluginsChanged` and by a dropped connection: the feed reloads list and snapshot. */
  epoch: number
  /** The epoch the current list and snapshot belong to (-1: none loaded). */
  loaded: number
  card: CommandCard | null
}

export const EMPTY_PLUGINS: PluginsState = { plugins: null, status: {}, revision: {}, invalidated: {}, contents: {}, epoch: 0, loaded: -1, card: null }

/** One `plugins.ui` event applied (pure). */
export function reduce(s: PluginsState, ev: PluginUi): PluginsState {
  switch (ev.type) {
    case 'PluginStatus': {
      const k = placementKey(ev.plugin, ev.placement)
      const prev = s.status[k]
      const next: SurfaceSnapshot = {
        plugin: ev.plugin,
        placement: ev.placement,
        revision: prev?.revision ?? 0,
        format: prev?.format ?? null,
        dot: ev.dot,
        badge: ev.badge,
        label: ev.label,
        tooltip: ev.tooltip,
      }
      return { ...s, status: { ...s.status, [k]: next } }
    }
    case 'PluginSurface': {
      const k = placementKey(ev.plugin, ev.placement)
      if ((s.revision[k] ?? 0) >= ev.revision) return s
      const prev = s.status[k]
      return {
        ...s,
        revision: { ...s.revision, [k]: ev.revision },
        status: prev ? { ...s.status, [k]: { ...prev, revision: ev.revision, format: ev.format } } : s.status,
      }
    }
    case 'PluginInvalidate': {
      const k = placementKey(ev.plugin, ev.placement)
      return { ...s, invalidated: { ...s.invalidated, [k]: (s.invalidated[k] ?? 0) + 1 } }
    }
    case 'PluginState':
      if (!s.plugins?.some((p) => p.name === ev.plugin)) return { ...s, epoch: s.epoch + 1 }
      return { ...s, plugins: s.plugins.map((p) => (p.name === ev.plugin ? { ...p, state: ev.state, error: ev.error } : p)) }
    case 'PluginsChanged':
      return { ...s, epoch: s.epoch + 1 }
  }
}

/** The list and snapshot of `epoch` taken in: the snapshot replaces the statuses; announced revisions only grow. */
export function seed(s: PluginsState, epoch: number, plugins: readonly PluginInfo[], surfaces: readonly SurfaceSnapshot[]): PluginsState {
  const status: Record<string, SurfaceSnapshot> = {}
  const revision: Record<string, number> = { ...s.revision }
  for (const snap of surfaces) {
    const k = placementKey(snap.plugin, snap.placement)
    status[k] = snap
    if (snap.revision > (revision[k] ?? 0)) revision[k] = snap.revision
  }
  return { ...s, plugins, status, revision, loaded: epoch }
}

const STORES = new WeakMap<Stores, StoreApi<PluginsState>>()

/** The plugins store of engine `st`. */
export function pluginStore(st: Stores = active()): StoreApi<PluginsState> {
  let store = STORES.get(st)
  if (!store) STORES.set(st, (store = createStore<PluginsState>()(() => EMPTY_PLUGINS)))
  return store
}

/** The enclosing engine's plugins store, selected. */
export function usePlugins<U>(select: (s: PluginsState) => U): U {
  return useStore(pluginStore(useScope()), select)
}

// The feed reaches only the active engine's listeners (`dispatch`), so they write the active engine's store.
const apply = (ev: PluginUi) => pluginStore().setState((s) => reduce(s, ev))
on('plugins.ui@1', 'PluginStatus', apply)
on('plugins.ui@1', 'PluginSurface', apply)
on('plugins.ui@1', 'PluginInvalidate', apply)
on('plugins.ui@1', 'PluginState', apply)
on('plugins.ui@1', 'PluginsChanged', apply)

const LOADING = new WeakSet<StoreApi<PluginsState>>()

/**
 * Keep the list and snapshot loaded while mounted: on first use, after `PluginsChanged`, after a reconnect. Every
 * placement hook mounts it; one load runs at a time per engine.
 */
function usePluginsFeed(): StoreApi<PluginsState> {
  const store = pluginStore(useScope())
  const api = useConnection((s) => s.api)
  const open = useConnection((s) => s.state === 'open')
  const able = useConnection((s) => s.discovery.phase === 'legacy' || supports(s.discovery, { methods: ['plugin_list', 'plugin_surfaces'] }))
  const epoch = useStore(store, (s) => s.epoch)
  const loaded = useStore(store, (s) => s.loaded)
  useEffect(() => {
    // A drop may have hidden events: the next open reloads.
    if (!open) {
      if (loaded === epoch) store.setState({ epoch: epoch + 1 })
      return
    }
    if (!api || !able || loaded === epoch || LOADING.has(store)) return
    LOADING.add(store)
    Promise.all([api.plugin_list(), api.plugin_surfaces()]).then(
      ([plugins, surfaces]) => store.setState((s) => seed(s, epoch, plugins, surfaces)),
      // Not running (disabled, older engine): nothing to draw; no retry until the next change or reconnect.
      () => store.setState((s) => seed(s, epoch, s.plugins ?? [], [])),
    ).finally(() => LOADING.delete(store))
  }, [store, api, open, able, epoch, loaded])
  return store
}

/** A plugin whose placements are drawn: enabled and past approval (a dead one still shows, dot overridden by the host). */
const drawn = (p: PluginInfo) => p.enabled && p.state !== 'disabled' && p.state !== 'invalid' && p.state !== 'awaiting_approval'

const byOrder = (a: PluginInfo, b: PluginInfo) => a.order - b.order || a.name.localeCompare(b.name)

export type PlacementEntry = { plugin: PluginInfo; placement: PlacementInfo; status: SurfaceSnapshot | undefined }

/** The dot an entry draws: its status's, `off` until the first status; none for a sidebar icon declared without one. */
export const entryDot = (e: PlacementEntry): Dot => (e.placement.kind === 'status' || e.placement.status ? (e.status?.dot ?? 'off') : 'none')

/** An overflow button's dot: the worst `warn`/`error` among the entries it holds (216 §9.1). */
export function overflowDot(entries: readonly PlacementEntry[]): Dot {
  const dots = entries.map(entryDot)
  return dots.includes('error') ? 'error' : dots.includes('warn') ? 'warn' : 'none'
}

/** A badge as drawn; a zero count or empty text hides it. */
export const badgeText = (b: Badge | null | undefined): string | null => (b === null || b === undefined || b === 0 || b === '' ? null : String(b))

/** Tooltip and menu text: the declared label, the plugin's label when it differs, its tooltip (or the state the host put there). */
export function entryTip(e: PlacementEntry): string {
  const label = e.status?.label
  return [e.placement.label, label !== e.placement.label ? label : null, e.status?.tooltip].filter(Boolean).join(' · ')
}

/** The shown placements of `kind`, in `PluginInfo.order`; hidden ones left out (pure). */
export function placementsOf(plugins: readonly PluginInfo[] | null, status: Readonly<Record<string, SurfaceSnapshot>>, kind: PlacementKind): PlacementEntry[] {
  if (!plugins) return []
  return [...plugins]
    .filter(drawn)
    .sort(byOrder)
    .flatMap((plugin) =>
      plugin.placements.filter((p) => p.kind === kind && !p.hidden).map((placement) => ({ plugin, placement, status: status[placementKey(plugin.name, placement.id)] })),
    )
}

export type PluginCommand = { plugin: string; command: CommandInfo }

/** Every runnable plugin's `/command`s, hidden ones included (a hidden placement's command still works) (pure). */
export function commandsOf(plugins: readonly PluginInfo[] | null): PluginCommand[] {
  if (!plugins) return []
  return [...plugins]
    .filter(drawn)
    .sort(byOrder)
    .flatMap((p) => p.placements.flatMap((pl) => (pl.kind === 'command' && pl.command ? [{ plugin: p.name, command: pl.command }] : [])))
}

/** The shown placements of `kind` (contract §12.2). */
export function usePluginPlacements(kind: PlacementKind): PlacementEntry[] {
  const store = usePluginsFeed()
  const plugins = useStore(store, (s) => s.plugins)
  const status = useStore(store, (s) => s.status)
  return useMemo(() => placementsOf(plugins, status, kind), [plugins, status, kind])
}

/** The `/command`s of running plugins (contract §12.2). */
export function usePluginCommands(): PluginCommand[] {
  const store = usePluginsFeed()
  const plugins = useStore(store, (s) => s.plugins)
  return useMemo(() => commandsOf(plugins), [plugins])
}

/** One declared placement and its plugin, from the loaded list (a tile's title, a surface's icon). */
export function usePluginPlacement(plugin: string, placement: string): { plugin: PluginInfo; placement: PlacementInfo } | undefined {
  const store = usePluginsFeed()
  const plugins = useStore(store, (s) => s.plugins)
  return useMemo(() => {
    const p = plugins?.find((x) => x.name === plugin)
    const pl = p?.placements.find((x) => x.id === placement)
    return p && pl ? { plugin: p, placement: pl } : undefined
  }, [plugins, plugin, placement])
}

const failed = (plugin: string, placement: string, e: unknown): SurfaceContent => ({
  plugin,
  placement,
  revision: 0,
  state: 'error',
  format: null,
  source: null,
  text: null,
  bindings: [],
  error: e instanceof Error ? e.message : String(e),
})

/**
 * One placement's content while shown (contract §12.2): fetched on mount (the host asks a lazy plugin to render and
 * answers `loading`), again whenever a newer revision is announced, and on `refresh()`.
 */
export function usePluginSurface(plugin: string, placement: string, session: string | null): { content: SurfaceContent | undefined; refresh(): void } {
  const store = pluginStore(useScope())
  const k = placementKey(plugin, placement)
  const content = useStore(store, (s) => s.contents[k])
  const announced = useStore(store, (s) => s.revision[k] ?? 0)
  const api = useConnection((s) => s.api)
  const [nonce, setNonce] = useState(0)
  useEffect(() => {
    if (!api) return
    let live = true
    const put = (c: SurfaceContent) => live && store.setState((s) => ({ contents: { ...s.contents, [k]: c } }))
    api.plugin_surface(plugin, placement, session).then(put, (e: unknown) => put(failed(plugin, placement, e)))
    return () => {
      live = false
    }
  }, [store, api, plugin, placement, session, k, announced, nonce])
  return { content, refresh: () => setNonce((n) => n + 1) }
}

/** `PluginInvalidate`s seen for one placement: a shown OpenUI surface re-runs its queries when this moves. */
export const usePluginInvalidated = (plugin: string, placement: string): number => usePlugins((s) => s.invalidated[placementKey(plugin, placement)] ?? 0)

/** Show a command's reply card (above the composer); `null` closes it. */
export const setCommandCard = (card: CommandCard | null): void => pluginStore().setState({ card })
