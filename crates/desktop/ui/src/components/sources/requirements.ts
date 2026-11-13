import type { Requirements } from '@/rpc/discovery'

export const requirements = {
  "methods": [
    "cell_ask",
    "cell_input",
    "cell_interpreters",
    "cell_latest",
    "cell_output",
    "cell_output_file",
    "cell_run",
    "cell_run_get",
    "cell_runs",
    "cell_state",
    "cell_stop",
    "doc_awareness",
    "doc_delete",
    "doc_link_create",
    "doc_link_revoke",
    "doc_member_revoke",
    "doc_member_set",
    "doc_members",
    "doc_open",
    "doc_resolve",
    "doc_subscribe",
    "doc_unsubscribe",
    "doc_update",
    "insight_promote",
    "insight_run",
    "insight_skills",
    "mesh_status",
    "notebook_get",
    "notebook_item_remove",
    "notebook_items_list",
    "notebook_list",
    "put_attachment",
    "put_session_attachment",
    "redo",
    "search",
    "source_get",
    "source_refresh",
    "undo"
  ],
  "topics": [
    "cells.events@1",
    "document.cell-latest@1"
  ]
} as const satisfies Requirements
