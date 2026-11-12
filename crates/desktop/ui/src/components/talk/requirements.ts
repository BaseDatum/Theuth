import type { Requirements } from '@/rpc/discovery'

export const requirements = {
  "methods": [
    "talk_audio",
    "talk_start",
    "talk_status",
    "talk_stop"
  ],
  "topics": [
    "talk.events@1",
    "voice.events@1"
  ]
} as const satisfies Requirements
