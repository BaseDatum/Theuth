import type { Requirements } from '@/rpc/discovery'

export const searchRequirements = {
  "methods": [
    "cancel",
    "list_sessions",
    "search",
    "search_refresh"
  ],
  "topics": []
} as const satisfies Requirements
