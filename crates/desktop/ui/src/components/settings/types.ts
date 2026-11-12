import type { LucideIcon } from 'lucide-react'
import type { ComponentType } from 'react'
import type { Requirements } from '@/rpc/discovery'

/** 188: what every settings tab receives. `focus` is the route's `?focus=` (Memory: `model` | `spend`). */
export type TabProps = { focus: string | null }

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
}

/** 188: one entry of the categories column; its tabs fill the section band. */
export type SettingsCategory = {
  id: string
  label: string
  icon: LucideIcon
  tabs: readonly [SettingsTab, ...SettingsTab[]]
}
