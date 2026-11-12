import type { Requirements } from '@/rpc/discovery'

export const requirements = {
  "methods": [
    "brief_delete",
    "brief_list",
    "brief_put",
    "brief_search",
    "brief_tags"
  ],
  "topics": [
    "meeting.briefs@1"
  ]
} as const satisfies Requirements
