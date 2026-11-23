import { Globe } from 'lucide-react'
import { SharingTab } from '@/components/settings/Sharing'
import type { SettingsCategory } from '@/components/settings/types'

// 210 §7.4 item 4: Settings › Sharing, on every client (iOS and web get it from the same bundle, item 10).
// TODO(210-M1, WIRE1): `required: {methods: ['share_settings_get', …]}` once the share methods are in the generated
// `MethodMap`; until then the tab says so itself when the engine lacks them.
export const sharing: SettingsCategory = {
  id: 'sharing',
  label: 'Sharing',
  icon: Globe,
  tabs: [{ id: 'sharing', label: 'Public sharing', icon: Globe, Component: SharingTab }],
}
