// Per-machine Settings (decision theuth-settings-machine-picker-design-2026-11-27), piece 1: every tab says where its
// data lives (`scope`), what the engine's host must have for it to work (`needs`), and whether a later
// "Set on all machines" may copy it (`copyable`). The machine picker reads the same fields; nothing here calls an engine.

import { container } from '@/lib/platform'

/**
 * Where a tab's data lives.
 * - `machine`: the connected engine's own config; another machine keeps its own (the picker will switch engines).
 * - `everywhere`: replicated across the mesh; any engine shows the same.
 * - `hub`: runs on the hub engine only (the lease holder).
 * - `device`: this client (browser storage, the shell); no engine involved.
 */
export type SettingsScope = 'machine' | 'everywhere' | 'hub' | 'device'

/** What the engine's host must have for a tab to work: a display, an interactive (logged-in) user, a microphone. */
export type SettingsNeed = 'display' | 'user' | 'mic'

/** The quiet tag's text (the mock's legend). */
export const SCOPE_LABEL: Readonly<Record<SettingsScope, string>> = {
  machine: 'machine',
  everywhere: 'everywhere',
  hub: 'hub only',
  device: 'this device',
}

/** Names the tooltips fill in; `null` when not known yet. */
export type ScopeNames = { engine: string | null; hub: string | null; device: string }

export function scopeTooltip(scope: SettingsScope, names: ScopeNames): string {
  switch (scope) {
    case 'machine':
      return `Saved on ${names.engine ?? 'this machine'}. Other machines keep their own.`
    case 'everywhere':
      return 'Shared by every machine in your mesh.'
    case 'hub':
      return names.hub ? `Runs on the hub (${names.hub}).` : 'Runs on the hub.'
    case 'device':
      return `Only on this ${names.device}.`
  }
}

/** This client's kind, for "This iPhone" and "Only on this Mac". */
export function deviceKind(): string {
  const c = container()
  if (c === 'ios') return typeof navigator !== 'undefined' && /iPhone/.test(navigator.userAgent) ? 'iPhone' : 'iPad'
  if (c === 'macos') return 'Mac'
  if (c === 'linux' || c === 'windows') return 'PC'
  return 'browser'
}

/** What an engine's host has: `true`/`false` when known, absent when not (an unknown is assumed present). */
export type EngineCaps = Partial<Record<SettingsNeed, boolean>>

/** `engine_caps()`'s answer (227 §1), the part `engineCaps` reads. */
export type HostCaps = { display: boolean; mic: boolean; interactive_user: boolean }

/**
 * An engine's caps.
 * - `host` (its `engine_caps` answer, 227) wins when known.
 * - Else the desktop shell's own daemon runs in the user's session: display, user and mic all present.
 * - Else a `list_input_devices` answer with no device means no mic; display and user stay unknown (an engine without
 *   `engine_caps`).
 */
export function engineCaps({ local, inputDevices, host }: { local: boolean; inputDevices?: readonly unknown[]; host?: HostCaps }): EngineCaps {
  if (host) return { display: host.display, user: host.interactive_user, mic: host.mic }
  if (local) return { display: true, user: true, mic: true }
  return inputDevices ? { mic: inputDevices.length > 0 } : {}
}

/** The needs of `tab` this engine lacks, or `null` when the tab applies here (unknown caps count as present). */
export function notApplicable(tab: { needs?: readonly SettingsNeed[] }, caps: EngineCaps): SettingsNeed[] | null {
  const missing = (tab.needs ?? []).filter((need) => caps[need] === false)
  return missing.length > 0 ? missing : null
}

const LACKS: Readonly<Record<SettingsNeed, string>> = { display: 'display', user: 'signed-in user', mic: 'microphone' }

/** The explanation an n/a tab shows: "P50 has no microphone, so this does nothing there." */
export function notApplicableText(missing: readonly SettingsNeed[], engine: string | null): string {
  const what = missing.map((need) => LACKS[need])
  const list = what.length > 1 ? `${what.slice(0, -1).join(', ')} or ${what.at(-1)}` : (what[0] ?? '')
  return `${engine ?? 'This machine'} has no ${list}, so this does nothing there.`
}

/** The one scope every tab shares, else `null` (a category row's tag). */
export function sharedScope(tabs: readonly { scope: SettingsScope }[]): SettingsScope | null {
  const first = tabs[0]?.scope
  return first && tabs.every((tab) => tab.scope === first) ? first : null
}
