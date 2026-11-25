import { FileText, MessageSquarePlus } from 'lucide-react'
import { Suffixes } from '@/components/settings/Suffixes'
import { SystemPrompt } from '@/components/settings/SystemPrompt'
import type { SettingsCategory } from '@/components/settings/types'

// #3: no engine requirement on Suffixes: an engine with the suffix methods keeps them mesh-wide, any other this device (`lib/suffixes`).
export const prompt: SettingsCategory = {
  id: 'prompt',
  label: 'Prompt',
  icon: MessageSquarePlus,
  tabs: [
    { id: 'suffixes', label: 'Suffixes', icon: MessageSquarePlus, Component: Suffixes },
    {
      id: 'system',
      label: 'System',
      icon: FileText,
      Component: SystemPrompt,
      required: {
        methods: ['list_prompt_sections', 'set_prompt_section', 'set_prompt_section_enabled', 'reset_prompt_section'],
        topics: [],
      },
    },
  ],
}
