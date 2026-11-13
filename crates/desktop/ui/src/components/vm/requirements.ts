import type { Requirements } from '@/rpc/discovery'

export const requirements = {
  "methods": [
    "handoff_resolve",
    "handoffs_list",
    "list_sessions",
    "sandbox_create",
    "sandbox_desktop_access",
    "sandbox_destroy",
    "sandbox_install_runtime",
    "sandbox_pause",
    "sandbox_resize",
    "sandbox_restore",
    "sandbox_resume",
    "sandbox_retry",
    "sandbox_set_shares",
    "sandbox_snapshot",
    "sandbox_start",
    "sandbox_stop",
    "sandbox_takeover",
    "sandboxes_list"
  ],
  "topics": [
    "sandbox.events@1"
  ]
} as const satisfies Requirements
