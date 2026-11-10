import { AppWindow, Keyboard, Palette, Settings2 } from 'lucide-react'
import { General } from '@/components/settings/General'
import { Shortcuts } from '@/components/settings/Shortcuts'
import type { SettingsCategory } from '@/components/settings/types'

// 188 C0: each tab renders one of today's components until this category's worker splits it
// (docs/design/188-settings-full-page.md, Contract). Only that worker edits this file.
export const general: SettingsCategory = {
  id: 'general',
  label: 'General',
  icon: Settings2,
  tabs: [
    { id: 'appearance', label: 'Appearance', icon: Palette, Component: General },
    { id: 'app', label: 'App', icon: AppWindow, Component: General },
    { id: 'shortcuts', label: 'Shortcuts', icon: Keyboard, Component: Shortcuts },
  ],
}
