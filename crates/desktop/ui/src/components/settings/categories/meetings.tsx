import { LayoutTemplate, Radar, Radio, SlidersHorizontal, Users } from 'lucide-react'
import { MeetingDefaults, MeetingDetection, MeetingLive, MeetingSpeakers } from '@/components/settings/Meetings'
import { MeetingTemplates } from '@/components/settings/MeetingTemplates'
import type { SettingsCategory } from '@/components/settings/types'

// 188: Meetings is its own category; one component per tab. Its two hotkeys live in General › Shortcuts. Only W-VOICE edits this file.
export const meetings: SettingsCategory = {
  id: 'meetings',
  label: 'Meetings',
  icon: Users,
  tabs: [
    { id: 'meetings', label: 'Defaults', icon: SlidersHorizontal, Component: MeetingDefaults },
    { id: 'detection', label: 'Detection', icon: Radar, Component: MeetingDetection },
    { id: 'live', label: 'Live', icon: Radio, Component: MeetingLive },
    { id: 'speakers', label: 'Speakers', icon: Users, Component: MeetingSpeakers },
    { id: 'templates', label: 'Templates', icon: LayoutTemplate, Component: MeetingTemplates },
  ],
}
