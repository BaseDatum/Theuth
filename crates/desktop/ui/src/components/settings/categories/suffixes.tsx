import { MessageSquarePlus } from 'lucide-react'
import { Suffixes } from '@/components/settings/Suffixes'
import type { SettingsCategory } from '@/components/settings/types'

// #3: client-side only (localStorage), so no engine requirement.
export const suffixes: SettingsCategory = {
  id: 'suffixes',
  label: 'Prompt suffixes',
  icon: MessageSquarePlus,
  tabs: [{ id: 'suffixes', label: 'Prompt suffixes', icon: MessageSquarePlus, Component: Suffixes }],
}
