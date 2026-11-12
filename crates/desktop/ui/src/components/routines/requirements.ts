import type { Requirements } from '@/rpc/discovery'

export const requirements = {
  "methods": [
    "autoresearch.control",
    "autoresearch.list",
    "dreaming.status",
    "enable_schedule",
    "fire_schedule",
    "list_schedules",
    "list_sensors",
    "mesh_status",
    "test_schedule"
  ],
  "topics": [
    "agent.events@1",
    "dreaming.finished@1",
    "dreaming.started@1",
    "dreaming.undone@1",
    "schedule.events@1"
  ]
} as const satisfies Requirements

export const panelRequirements = {
  "methods": [
    "autoresearch.control",
    "autoresearch.list",
    "delete_schedule",
    "dreaming.status",
    "enable_schedule",
    "fire_schedule",
    "get_schedule",
    "list_schedules",
    "list_secrets",
    "list_sensors",
    "mesh_status",
    "new_session",
    "put_schedule",
    "rename_session",
    "schedule_history",
    "test_schedule",
    "test_schedule_file",
    "use_skill"
  ],
  "topics": [
    "agent.events@1",
    "dreaming.finished@1",
    "dreaming.started@1",
    "dreaming.undone@1",
    "schedule.events@1"
  ]
} as const satisfies Requirements
