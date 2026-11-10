import { LayoutTemplate, Radar, Radio, SlidersHorizontal, Users } from 'lucide-react'
import { Meetings } from '@/components/settings/Meetings'
import type { SettingsCategory } from '@/components/settings/types'

// 188 C0: each tab renders one of today's components until this category's worker splits it
// (docs/design/188-settings-full-page.md, Contract). Only that worker edits this file.
export const meetings: SettingsCategory = {
  id: 'meetings',
  label: 'Meetings',
  icon: Users,
  tabs: [
    { id: 'meetings', label: 'Defaults', icon: SlidersHorizontal, Component: Meetings },
    { id: 'detection', label: 'Detection', icon: Radar, Component: Meetings },
    { id: 'live', label: 'Live', icon: Radio, Component: Meetings },
    { id: 'speakers', label: 'Speakers', icon: Users, Component: Meetings },
    { id: 'templates', label: 'Templates', icon: LayoutTemplate, Component: Meetings },
  ],
}
