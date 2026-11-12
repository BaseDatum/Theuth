import type { Requirements } from '@/rpc/discovery'

export const requirements = {
  "methods": [
    "knowledge_history",
    "knowledge_list",
    "knowledge_restore",
    "knowledge_revision",
    "knowledge_search",
    "knowledge_sync"
  ],
  "topics": [
    "knowledge.changed@1"
  ]
} as const satisfies Requirements

export const panelRequirements = {
  "methods": [
    "doc_undo_agent",
    "knowledge_delete",
    "knowledge_get",
    "knowledge_history",
    "knowledge_list",
    "knowledge_read_revision",
    "knowledge_restore",
    "knowledge_revision",
    "knowledge_search",
    "knowledge_sync",
    "knowledge_write"
  ],
  "topics": [
    "knowledge.changed@1"
  ]
} as const satisfies Requirements
