import type { Requirements } from '@/rpc/discovery'

export const requirements = {
  "methods": [
    "doc_create",
    "doc_delete",
    "doc_list",
    "doc_rename"
  ],
  "topics": [
    "document.changed@1"
  ]
} as const satisfies Requirements

export const panelRequirements = {
  "methods": [
    "doc_awareness",
    "doc_create",
    "doc_delete",
    "doc_export",
    "doc_list",
    "doc_markdown",
    "doc_move_home",
    "doc_open",
    "doc_rename",
    "doc_restore",
    "doc_snapshots",
    "doc_subscribe",
    "doc_unsubscribe",
    "doc_update",
    "list_sessions",
    "load_session"
  ],
  "topics": [
    "document.changed@1"
  ]
} as const satisfies Requirements
