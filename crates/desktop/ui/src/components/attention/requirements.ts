import type { Requirements } from '@/rpc/discovery'

export const requirements = {
  "methods": [
    "attention_list",
    "double_forget",
    "double_take_over",
    "list_sessions",
    "mesh_status",
    "tasks_list"
  ],
  "topics": [
    "roster.attention@1"
  ]
} as const satisfies Requirements

export const panelRequirements = {
  "methods": [
    "attention_list",
    "double_forget",
    "double_take_over",
    "get_artifact",
    "list_sessions",
    "load_session",
    "mesh_status",
    "packet_get",
    "tasks_list"
  ],
  "topics": [
    "roster.attention@1"
  ]
} as const satisfies Requirements
