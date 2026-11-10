import { AppWindow, Keyboard, Palette, Settings2 } from 'lucide-react'
import { App } from '@/components/settings/App'
import { Appearance } from '@/components/settings/Appearance'
import { Shortcuts } from '@/components/settings/Shortcuts'
import type { SettingsCategory } from '@/components/settings/types'

// 188: tab ids, labels, icons and order are frozen by C0 (docs/design/188-settings-full-page.md, Contract).
export const general: SettingsCategory = {
  id: 'general',
  label: 'General',
  icon: Settings2,
  tabs: [
    { id: 'appearance', label: 'Appearance', icon: Palette, Component: Appearance },
    { id: 'app', label: 'App', icon: AppWindow, Component: App },
    { id: 'shortcuts', label: 'Shortcuts', icon: Keyboard, Component: Shortcuts },
  ],
}
