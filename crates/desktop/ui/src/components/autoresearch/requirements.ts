import type { Requirements } from '@/rpc/discovery'

export const requirements = {
  "methods": [
    "autoresearch.discard_draft",
    "autoresearch.drafts",
    "autoresearch.list",
    "list_schedules",
    "send_input"
  ],
  "topics": [
    "autoresearch.events@1"
  ]
} as const satisfies Requirements

export const panelRequirements = {
  "methods": [
    "autoresearch.control",
    "autoresearch.create",
    "autoresearch.discard_draft",
    "autoresearch.draft",
    "autoresearch.draft_confirm",
    "autoresearch.draft_edit",
    "autoresearch.drafts",
    "autoresearch.experiment",
    "autoresearch.experiments",
    "autoresearch.get",
    "autoresearch.human_pair",
    "autoresearch.idea",
    "autoresearch.judgments",
    "autoresearch.list",
    "autoresearch.mechanism",
    "autoresearch.settings.get",
    "autoresearch.steer",
    "create_session",
    "list_schedules",
    "rename_session",
    "send_input",
    "use_skill"
  ],
  "topics": [
    "autoresearch.events@1"
  ]
} as const satisfies Requirements
