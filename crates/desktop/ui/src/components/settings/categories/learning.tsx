import { Brain, Moon, Sparkles, UserRoundCheck } from 'lucide-react'
import { Double } from '@/components/settings/Double'
import { Dreaming } from '@/components/settings/Dreaming'
import { Labs } from '@/components/settings/Labs'
import type { SettingsCategory } from '@/components/settings/types'

// 188 C0: each tab renders one of today's components until this category's worker splits it
// (docs/design/188-settings-full-page.md, Contract). Only that worker edits this file.
export const learning: SettingsCategory = {
  id: 'learning',
  label: 'Learning',
  icon: Sparkles,
  tabs: [
    { id: 'dreaming', label: 'Dreaming', icon: Moon, Component: Dreaming },
    { id: 'double', label: 'Double', icon: UserRoundCheck, Component: Double },
    { id: 'decision', label: 'Decision models', icon: Brain, Component: Labs },
  ],
}
