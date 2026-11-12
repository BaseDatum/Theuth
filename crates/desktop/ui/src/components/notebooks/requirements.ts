import type { Requirements } from '@/rpc/discovery'

export const requirements = {
  "methods": [
    "doc_markdown",
    "notebook_ask",
    "notebook_ask_list",
    "notebook_create",
    "notebook_delete",
    "notebook_delete_preview",
    "notebook_list",
    "notebook_update"
  ],
  "topics": [
    "agent.notebook-ask@1",
    "document.sources@1"
  ]
} as const satisfies Requirements

export const panelRequirements = {
  "methods": [
    "create_session",
    "doc_markdown",
    "endless_settings_get",
    "get_artifact",
    "load_session",
    "notebook_ask",
    "notebook_ask_list",
    "notebook_chats",
    "notebook_context_estimate",
    "notebook_context_set",
    "notebook_create",
    "notebook_delete",
    "notebook_delete_preview",
    "notebook_get",
    "notebook_item_add",
    "notebook_item_remove",
    "notebook_items_list",
    "notebook_list",
    "notebook_overview_run",
    "notebook_overviews",
    "notebook_update",
    "rename_session",
    "session_set_notebook",
    "source_get",
    "source_list"
  ],
  "topics": [
    "agent.notebook-ask@1",
    "document.sources@1"
  ]
} as const satisfies Requirements
