import { AudioLines, BookA, History, Mic, MicVocal } from 'lucide-react'
import type { SettingsCategory } from '@/components/settings/types'
import { Dictation, Microphone, VoiceHistory } from '@/components/settings/Voice'
import { VoiceDictionary } from '@/components/settings/VoiceDictionary'
import { VoiceKey } from '@/components/settings/VoiceKey'

// 188: Voice is its own category (Meetings and Calendar are separate); one component per tab. Only W-VOICE edits this file.
export const voice: SettingsCategory = { 
  id: 'voice',
  label: 'Voice',
  icon: Mic,
  tabs: [
    { required: {"methods":["list_input_devices","set_input_devices","voice_status"],"topics":[]}, id: 'voice', scope: 'machine', copyable: false, needs: ['mic', 'user'], copyNote: "The microphone is this machine's.", label: 'Microphone', icon: Mic, Component: Microphone },
    { required: {"methods":["list_models","polish_effort","polish_model","set_end_phrases","set_model_favorite","set_polish_effort","set_polish_enabled","set_polish_model","set_shape","set_shape_enabled","set_stt_engine","voice_mlx_download","voice_mlx_status","voice_settings"],"topics":[]}, id: 'dictation', scope: 'machine', copyable: false, needs: ['user'], copyNote: "Runs in this machine's session; the polish model may copy later.", label: 'Dictation', icon: MicVocal, Component: Dictation },
    { required: {"methods":["dictionary_delete","dictionary_list","dictionary_put"],"topics":[]}, id: 'voice-dictionary', scope: 'machine', copyable: true, label: 'Dictionary', icon: BookA, Component: VoiceDictionary },
    { required: {"methods":["desk_open","info","screen_context","send_input","steer"],"topics":[]}, id: 'voice-key', scope: 'machine', copyable: false, copyNote: "Bound to this machine's keyboard.", label: 'Voice key', icon: AudioLines, Component: VoiceKey },
    { required: {"methods":["dictation_audio_get","dictation_history","dictation_history_forget","dictation_status"],"topics":[]}, id: 'voice-history', scope: 'machine', copyable: false, copyNote: "Recordings stay on this machine.", label: 'History', icon: History, Component: VoiceHistory },
  ],
}
