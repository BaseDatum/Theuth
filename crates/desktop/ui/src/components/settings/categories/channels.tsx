import { Hash, MessageCircle, MessageSquare, Send } from 'lucide-react'
import { Channels } from '@/components/settings/Channels'
import type { SettingsCategory } from '@/components/settings/types'

// 188 C0: each tab renders one of today's components until this category's worker splits it
// (docs/design/188-settings-full-page.md, Contract). Only that worker edits this file.
export const channels: SettingsCategory = {
  id: 'channels',
  label: 'Channels',
  icon: MessageSquare,
  tabs: [
    { id: 'signal', label: 'Signal', icon: MessageCircle, Component: Channels },
    { id: 'slack', label: 'Slack', icon: Hash, Component: Channels },
    { id: 'telegram', label: 'Telegram', icon: Send, Component: Channels },
  ],
}
