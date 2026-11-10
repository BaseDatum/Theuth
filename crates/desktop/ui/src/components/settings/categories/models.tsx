import { Cloud, Cpu, Wand2 } from 'lucide-react'
import { Chat } from '@/components/settings/Chat'
import { Connections } from '@/components/settings/Connections'
import { Loop } from '@/components/settings/Loop'
import type { SettingsCategory } from '@/components/settings/types'

// 188 C0: each tab renders one of today's components until this category's worker splits it
// (docs/design/188-settings-full-page.md, Contract). Only that worker edits this file.
export const models: SettingsCategory = {
  id: 'models',
  label: 'Models',
  icon: Cpu,
  tabs: [
    { id: 'models', label: 'Defaults', icon: Cpu, Component: Chat },
    { id: 'helpers', label: 'Helpers', icon: Wand2, Component: Loop },
    { id: 'providers', label: 'Providers', icon: Cloud, Component: Connections },
  ],
}
