import type { Requirements } from '@/rpc/discovery'

export const requirements = {
  "methods": [
    "activity_list",
    "agent_approve",
    "agent_pause",
    "agent_remove",
    "answer_ask",
    "attention_dismiss",
    "attention_list",
    "attention_snooze",
    "contact_first_answer",
    "desk_open",
    "handoff_resolve",
    "list_sessions",
    "resolve_approval",
    "resume_turn",
    "secret_requests_list",
    "session_context_breakdown",
    "spend_report",
    "task_decide",
    "task_move",
    "task_move_home_waiting",
    "task_update"
  ],
  "topics": [
    "agent.events@1",
    "roster.attention@1",
    "tasks.activity@1"
  ]
} as const satisfies Requirements
