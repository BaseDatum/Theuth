import type { Requirements } from '@/rpc/discovery'

export const requirements = {
  "methods": [
    "get_artifact",
    "get_attachment",
    "list_artifacts",
    "list_sessions"
  ],
  "topics": [
    "artifact.shown@1"
  ]
} as const satisfies Requirements

export const panelRequirements = {
  "methods": [
    "artifact_search_match",
    "get_artifact",
    "get_artifact_listing",
    "get_attachment",
    "list_artifacts",
    "list_sessions"
  ],
  "topics": [
    "artifact.shown@1"
  ]
} as const satisfies Requirements
