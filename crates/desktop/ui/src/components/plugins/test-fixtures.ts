// Test fixtures for the plugin placement tests (216 B4): a `PluginInfo` / `PlacementInfo` with sane defaults.
import type { PlacementInfo, PluginInfo, SurfaceSnapshot } from '@/rpc/bindings/method.plugins.output'

export const placement = (id: string, over: Partial<PlacementInfo> = {}): PlacementInfo => ({
  id,
  kind: 'sidebar',
  label: id,
  icon: 'gauge',
  status: true,
  open: 'popover',
  size: null,
  format: 'markdown',
  fallback: 'text',
  hidden: false,
  command: null,
  ...over,
})

export const pluginInfo = (name: string, placements: PlacementInfo[], over: Partial<PluginInfo> = {}): PluginInfo => ({
  name,
  version: '1',
  description: '',
  state: 'running',
  error: null,
  enabled: true,
  needs_approval: false,
  card: null,
  placements,
  tools: [],
  dir: `/p/${name}`,
  log: `/l/${name}`,
  approved_at: null,
  order: 0,
  ...over,
})

export const snap = (plugin: string, place: string, over: Partial<SurfaceSnapshot> = {}): SurfaceSnapshot => ({
  plugin,
  placement: place,
  dot: 'ok',
  badge: null,
  label: null,
  tooltip: null,
  revision: 0,
  format: null,
  ...over,
})
