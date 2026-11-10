import { AudioLines, History, Mic, MicVocal } from 'lucide-react'
import type { SettingsCategory } from '@/components/settings/types'
import { Voice } from '@/components/settings/Voice'
import { VoiceKey } from '@/components/settings/VoiceKey'

// 188 C0: each tab renders one of today's components until this category's worker splits it
// (docs/design/188-settings-full-page.md, Contract). Only that worker edits this file.
export const voice: SettingsCategory = {
  id: 'voice',
  label: 'Voice',
  icon: Mic,
  tabs: [
    { id: 'voice', label: 'Microphone', icon: Mic, Component: Voice },
    { id: 'dictation', label: 'Dictation', icon: MicVocal, Component: Voice },
    { id: 'voice-key', label: 'Voice key', icon: AudioLines, Component: VoiceKey },
    { id: 'voice-history', label: 'History', icon: History, Component: Voice },
  ],
}
