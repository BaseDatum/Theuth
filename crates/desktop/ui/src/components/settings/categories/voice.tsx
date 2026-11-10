import { AudioLines, History, Mic, MicVocal } from 'lucide-react'
import type { SettingsCategory } from '@/components/settings/types'
import { Dictation, Microphone, VoiceHistory } from '@/components/settings/Voice'
import { VoiceKey } from '@/components/settings/VoiceKey'

// 188: Voice is its own category (Meetings and Calendar are separate); one component per tab. Only W-VOICE edits this file.
export const voice: SettingsCategory = {
  id: 'voice',
  label: 'Voice',
  icon: Mic,
  tabs: [
    { id: 'voice', label: 'Microphone', icon: Mic, Component: Microphone },
    { id: 'dictation', label: 'Dictation', icon: MicVocal, Component: Dictation },
    { id: 'voice-key', label: 'Voice key', icon: AudioLines, Component: VoiceKey },
    { id: 'voice-history', label: 'History', icon: History, Component: VoiceHistory },
  ],
}
