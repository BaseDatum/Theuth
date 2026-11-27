import { CalendarClock, CalendarDays, FileText } from 'lucide-react'
import { CalendarBriefs, CalendarSources, CalendarUpcoming } from '@/components/settings/Calendar'
import type { SettingsCategory } from '@/components/settings/types'

// 188: Calendar is its own category; one component per tab. Only W-VOICE edits this file.
export const calendar: SettingsCategory = { 
  id: 'calendar',
  label: 'Calendar',
  icon: CalendarDays,
  tabs: [
    { required: {"methods":["calendar_settings_get","calendar_settings_set","calendar_source_add","calendar_source_delete","calendar_source_refresh","calendar_sources"],"topics":[]}, id: 'calendar', scope: 'machine', copyable: false, copyNote: "Calendar sources are per machine.", connectedOnly: true, label: 'Calendars', icon: CalendarDays, Component: CalendarSources },
    { required: {"methods":["calendar_settings_get","calendar_settings_set"],"topics":[]}, id: 'briefs', scope: 'machine', copyable: true, label: 'Briefs', icon: FileText, Component: CalendarBriefs },
    { required: {"methods":["calendar_events"],"topics":[]}, id: 'upcoming', scope: 'machine', copyable: false, copyNote: "Calendar sources are per machine.", label: 'Upcoming', icon: CalendarClock, Component: CalendarUpcoming },
  ],
}
