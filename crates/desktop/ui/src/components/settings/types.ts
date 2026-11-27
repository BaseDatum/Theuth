import type { LucideIcon } from 'lucide-react'
import type { ComponentType } from 'react'
import type { SettingsNeed, SettingsScope } from '@/components/settings/scope'
import type { Requirements } from '@/rpc/discovery'

/** 188: what every settings tab receives. `focus` is the route's `?focus=` (Memory: `model` | `spend`); `progress` is `?progress=` (Machines: cloud node name). */
export type TabProps = { focus: string | null; progress?: string | null }

/** 188: one tab of a category. Its `id` is its URL (`/settings/<id>`), unique across every category. */
export type SettingsTab = {
  id: string
  label: string
  /** The phone band shows icons only (97b). */
  icon: LucideIcon
  Component: ComponentType<TabProps>
  required?: Requirements
  /** Omitted: always shown. Return false to hide the tab here (a phone or web client has no local node); called during render, so it must not call hooks. */
  visible?: () => boolean
  /** Where its data lives (`scope.ts`); the nav and the tab header tag it. */
  scope: SettingsScope
  /** What the engine's host must have; a tab whose need the engine lacks reads "not on this machine". */
  needs?: readonly SettingsNeed[]
  /** Whether a later "Set on all machines" may copy its values to other engines; only `machine` tabs can be. */
  copyable: boolean
  /** Why a `machine` tab is not copyable. */
  copyNote?: string
  /** 227: its methods are not forwardable (`engine_call`), so it ignores the machine picker and edits the connected machine only. */
  connectedOnly?: boolean
}

/** 188: one entry of the categories column; its tabs fill the section band. */
export type SettingsCategory = {
  id: string
  label: string
  icon: LucideIcon
  tabs: readonly [SettingsTab, ...SettingsTab[]]
}
