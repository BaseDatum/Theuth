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
    { required: {"methods":["signal_allow","signal_link","signal_remove_allowed","signal_status","signal_unlink"],"topics":["signal.events@1"]}, id: 'signal', label: 'Signal', icon: MessageCircle, Component: Signal },
    { required: {"methods":["slack_allow","slack_allow_channel","slack_manifest","slack_remove_allowed","slack_remove_channel","slack_status"],"topics":["slack.events@1"]}, id: 'slack', label: 'Slack', icon: Hash, Component: Slack },
    { required: {"methods":["set_secret","telegram_allow","telegram_allow_group","telegram_link","telegram_remove_allowed","telegram_remove_group","telegram_status"],"topics":["telegram.events@1","telegram.events@1"]}, id: 'telegram', label: 'Telegram', icon: Send, Component: Telegram },
  ],
}
