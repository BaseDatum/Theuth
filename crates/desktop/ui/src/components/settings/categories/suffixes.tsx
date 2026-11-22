import { MessageSquarePlus } from 'lucide-react'
import { Suffixes } from '@/components/settings/Suffixes'
import type { SettingsCategory } from '@/components/settings/types'

// #3: no engine requirement: an engine with the suffix methods keeps them mesh-wide, any other this device (`lib/suffixes`).
export const suffixes: SettingsCategory = {
  id: 'suffixes',
  label: 'Prompt suffixes',
  icon: MessageSquarePlus,
  tabs: [{ id: 'suffixes', label: 'Prompt suffixes', icon: MessageSquarePlus, Component: Suffixes }],
}
