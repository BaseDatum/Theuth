import type { Requirements } from '@/rpc/discovery'

export const requirements = {
  "methods": [
    "agents_list",
    "cancel",
    "doc_list",
    "engine_agents_list",
    "get_attachment",
    "knowledge_list",
    "list_artifacts",
    "list_project_styles",
    "list_sessions",
    "meeting_list",
    "mesh_status",
    "search",
    "search_refresh",
    "task_create",
    "task_move",
    "task_move_home",
    "task_update",
    "tasks_list"
  ],
  "topics": [
    "agent.events@1",
    "tasks.activity@1"
  ]
} as const satisfies Requirements

export const panelRequirements = {
  "methods": [
    "agents_list",
    "cancel",
    "doc_list",
    "engine_agents_list",
    "get_attachment",
    "knowledge_list",
    "list_artifacts",
    "list_project_styles",
    "list_sessions",
    "meeting_list",
    "mesh_status",
    "search",
    "search_refresh",
    "session_cost",
    "task_comment",
    "task_create",
    "task_events",
    "task_get",
    "task_move",
    "task_move_home",
    "task_update",
    "tasks_list"
  ],
  "topics": [
    "agent.events@1",
    "tasks.activity@1"
  ]
} as const satisfies Requirements
