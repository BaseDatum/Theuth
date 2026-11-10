import { Cloud, Cpu, Wand2 } from 'lucide-react'
import { Helpers } from '@/components/settings/Helpers'
import { ModelDefaults } from '@/components/settings/ModelDefaults'
import { ModelProviders } from '@/components/settings/ModelProviders'
import type { SettingsCategory } from '@/components/settings/types'

// 188: tab ids, labels, icons and order are frozen by C0 (docs/design/188-settings-full-page.md, Contract).
export const models: SettingsCategory = {
  id: 'models',
  label: 'Models',
  icon: Cpu,
  tabs: [
    { id: 'models', label: 'Defaults', icon: Cpu, Component: ModelDefaults },
    { id: 'helpers', label: 'Helpers', icon: Wand2, Component: Helpers },
    { id: 'providers', label: 'Providers', icon: Cloud, Component: ModelProviders },
  ],
}
