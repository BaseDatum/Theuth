import type { Requirements } from '@/rpc/discovery'

export const requirements = {
  "methods": [
    "cancel",
    "codeindex_stats",
    "codemap_cancel",
    "codemap_change",
    "codemap_element",
    "codemap_fit",
    "codemap_generate",
    "codemap_here",
    "codemap_overview",
    "codemap_search",
    "codemap_touch",
    "put_attachment",
    "put_session_attachment"
  ],
  "topics": [
    "codeindex.events@1"
  ]
} as const satisfies Requirements
