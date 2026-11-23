import { Globe } from 'lucide-react'
import { SharingTab } from '@/components/settings/Sharing'
import type { SettingsCategory } from '@/components/settings/types'

// 210 §7.4 item 4: Settings › Sharing, on every client (iOS and web get it from the same bundle, item 10).
// Hidden on an engine without the share plugin (discovery lacks its methods).
export const sharing: SettingsCategory = {
  id: 'sharing',
  label: 'Sharing',
  icon: Globe,
  tabs: [{ id: 'sharing', label: 'Public sharing', icon: Globe, Component: SharingTab, required: { methods: ['share_settings_get', 'share_list'] } }],
}
