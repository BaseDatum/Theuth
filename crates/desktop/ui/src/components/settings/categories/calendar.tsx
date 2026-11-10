import { CalendarClock, CalendarDays, FileText } from 'lucide-react'
import { Calendar } from '@/components/settings/Calendar'
import type { SettingsCategory } from '@/components/settings/types'

// 188 C0: each tab renders one of today's components until this category's worker splits it
// (docs/design/188-settings-full-page.md, Contract). Only that worker edits this file.
export const calendar: SettingsCategory = {
  id: 'calendar',
  label: 'Calendar',
  icon: CalendarDays,
  tabs: [
    { id: 'calendar', label: 'Calendars', icon: CalendarDays, Component: Calendar },
    { id: 'briefs', label: 'Briefs', icon: FileText, Component: Calendar },
    { id: 'upcoming', label: 'Upcoming', icon: CalendarClock, Component: Calendar },
  ],
}
