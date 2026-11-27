import { Puzzle } from 'lucide-react'
import { Plugins } from '@/components/settings/Plugins'
import type { SettingsCategory } from '@/components/settings/types'

// 216 §9.6, contract §12.6: Plugins, the 13th category — external plugins (folders under plugins/, any language), their
// approval, state, placements and logs. An engine without `plugin_list` (or with the plugins host off) hides it.
export const plugins: SettingsCategory = {
  id: 'plugins',
  label: 'Plugins',
  icon: Puzzle,
  tabs: [
    { required: {"methods":["plugin_list","plugin_get","plugin_approve","plugin_set_enabled","plugin_set_hidden","plugin_set_order","plugin_restart","plugin_uninstall","plugin_logs","plugin_new"],"topics":["plugins.ui@1"]}, id: 'plugins', scope: 'machine', copyable: true, copyNote: "Only enabled/disabled copies.", label: 'Plugins', icon: Puzzle, Component: Plugins },
  ],
}
