import type { Requirements } from '@/rpc/discovery'

export const requirements = {
  "methods": [
    "agent_approve",
    "agent_pause",
    "agent_put",
    "agent_remove",
    "agents_list",
    "engine_agents_list",
    "list_agents",
    "list_sessions",
    "mesh_status",
    "session_cost",
    "tasks_list"
  ],
  "topics": [
    "agent.events@1"
  ]
} as const satisfies Requirements

export const panelRequirements = {
  "methods": [
    "agent_approve",
    "agent_get",
    "agent_pause",
    "agent_put",
    "agent_remove",
    "agents_list",
    "doc_ensure",
    "doc_markdown",
    "doc_resolve",
    "engine_agents_list",
    "list_acp",
    "list_agents",
    "list_mcp",
    "list_models",
    "list_sessions",
    "mesh_status",
    "session_cost",
    "set_theuth_agent_effort",
    "set_theuth_agent_model",
    "tasks_list",
    "theuth_agent_effort",
    "theuth_agent_model"
  ],
  "topics": [
    "agent.events@1"
  ]
} as const satisfies Requirements
