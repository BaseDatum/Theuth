import { Brain, Moon, Sparkles, UserRoundCheck } from 'lucide-react'
import { Double } from '@/components/settings/Double'
import { Dreaming } from '@/components/settings/Dreaming'
import { Labs } from '@/components/settings/Labs'
import type { SettingsCategory } from '@/components/settings/types'

// 188: Learning (docs/design/188-settings-full-page.md); Decision models is today's Labs, always listed (F4).
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
