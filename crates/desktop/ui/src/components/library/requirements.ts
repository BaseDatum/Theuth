import type { Requirements } from '@/rpc/discovery'

export const skillsRequirements = {
  "methods": [
    "add_skill",
    "add_skill_repo",
    "list_skills",
    "load_session",
    "load_skill",
    "remove_skill",
    "remove_skill_repo",
    "sync_skills"
  ],
  "topics": [
    "agent.events@1"
  ]
} as const satisfies Requirements

export const secretsRequirements = {
  "methods": [
    "delete_secret",
    "list_secrets",
    "restore_secret",
    "set_secret"
  ],
  "topics": [
    "secrets.events@1"
  ]
} as const satisfies Requirements

export const toolsRequirements = {
  "methods": [
    "add_mcp_repo",
    "add_mcp_server",
    "list_mcp",
    "mcp_login",
    "mcp_logout",
    "mcp_status",
    "remove_mcp_server",
    "set_mcp_gate",
    "set_mcp_hidden",
    "set_mcp_home",
    "sync_mcp"
  ],
  "topics": [
    "tools.mcp-progress@1"
  ]
} as const satisfies Requirements
