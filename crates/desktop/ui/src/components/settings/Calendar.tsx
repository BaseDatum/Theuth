// 158 W2 owns this file (stub from C158). Settings › Calendar (158 §7.6): macOS Calendar switch + state line,
// ICS subscriptions (add / refresh / delete), pre-meeting briefs switch, Upcoming list. Hidden from the
// tab list until W2 flips `ready`.

import { Empty } from '@/components/settings/shared'

/** `SettingsDialog` drops this tab while `false`; W2 flips it with the real panel. */
export const ready = false

export function Calendar() {
  return <Empty>Calendar settings arrive with 158 W2.</Empty>
}
