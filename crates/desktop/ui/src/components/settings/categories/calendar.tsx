import { CalendarClock, CalendarDays, FileText } from 'lucide-react'
import { CalendarBriefs, CalendarSources, CalendarUpcoming } from '@/components/settings/Calendar'
import type { SettingsCategory } from '@/components/settings/types'

// 188: Calendar is its own category; one component per tab. Only W-VOICE edits this file.
export const calendar: SettingsCategory = {
  id: 'calendar',
  label: 'Calendar',
  icon: CalendarDays,
  tabs: [
    { id: 'calendar', label: 'Calendars', icon: CalendarDays, Component: CalendarSources },
    { id: 'briefs', label: 'Briefs', icon: FileText, Component: CalendarBriefs },
    { id: 'upcoming', label: 'Upcoming', icon: CalendarClock, Component: CalendarUpcoming },
  ],
}
