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
    { required: {"methods":["list_models","list_project_styles","list_sessions","meeting_settings_get","meeting_settings_set","meeting_templates","set_model_favorite"],"topics":[]}, id: 'meetings', label: 'Defaults', icon: SlidersHorizontal, Component: MeetingDefaults },
    { required: {"methods":["meeting_settings_get","meeting_settings_set"],"topics":[]}, id: 'detection', label: 'Detection', icon: Radar, Component: MeetingDetection },
    { required: {"methods":["meeting_get","meeting_list","meeting_live_diarization","meeting_settings_get","meeting_settings_set","set_meeting_live_diarization"],"topics":[]}, id: 'live', label: 'Live', icon: Radio, Component: MeetingLive },
    { required: {"methods":["meeting_speaker_delete","meeting_speakers"],"topics":[]}, id: 'speakers', label: 'Speakers', icon: Users, Component: MeetingSpeakers },
    { required: {"methods":["meeting_template_delete","meeting_template_put","meeting_template_reset","meeting_templates"],"topics":[]}, id: 'templates', label: 'Templates', icon: LayoutTemplate, Component: MeetingTemplates },
  ],
}
