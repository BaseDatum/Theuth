import { Hash, MessageCircle, MessageSquare, Send } from 'lucide-react'
import { Signal } from '@/components/settings/Signal'
import { Slack } from '@/components/settings/Slack'
import { Telegram } from '@/components/settings/Telegram'
import type { SettingsCategory } from '@/components/settings/types'

// 188: one component per channel (docs/design/188-settings-full-page.md).
export const channels: SettingsCategory = {
  id: 'channels',
  label: 'Channels',
  icon: MessageSquare,
  tabs: [
    { id: 'signal', label: 'Signal', icon: MessageCircle, Component: Signal },
    { id: 'slack', label: 'Slack', icon: Hash, Component: Slack },
    { id: 'telegram', label: 'Telegram', icon: Send, Component: Telegram },
  ],
}
