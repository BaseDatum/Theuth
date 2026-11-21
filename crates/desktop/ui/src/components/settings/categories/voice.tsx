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
    { required: {"methods":["list_input_devices","set_input_devices","voice_status"],"topics":[]}, id: 'voice', label: 'Microphone', icon: Mic, Component: Microphone },
    { required: {"methods":["list_models","polish_effort","polish_model","set_end_phrases","set_model_favorite","set_polish_effort","set_polish_model","set_shape","set_shape_enabled","set_stt_engine","voice_mlx_download","voice_mlx_status","voice_settings"],"topics":[]}, id: 'dictation', label: 'Dictation', icon: MicVocal, Component: Dictation },
    { required: {"methods":["desk_open","info","screen_context","send_input","steer"],"topics":[]}, id: 'voice-key', label: 'Voice key', icon: AudioLines, Component: VoiceKey },
    { required: {"methods":["dictation_audio_get","dictation_history","dictation_history_forget","dictation_status"],"topics":[]}, id: 'voice-history', label: 'History', icon: History, Component: VoiceHistory },
  ],
}
