import type { Requirements } from '@/rpc/discovery'

export const requirements = {
  "methods": [
    "calendar_events",
    "list_project_styles",
    "list_sessions",
    "meeting_delete",
    "meeting_detection",
    "meeting_detection_dismiss",
    "meeting_detection_set_event",
    "meeting_get",
    "meeting_list",
    "meeting_set_event",
    "meeting_start",
    "meeting_status",
    "meeting_stop"
  ],
  "topics": [
    "meeting.events@1"
  ]
} as const satisfies Requirements

export const panelRequirements = {
  "methods": [
    "calendar_events",
    "decision_mode",
    "endless_settings_get",
    "list_models",
    "list_project_styles",
    "list_sessions",
    "meeting_briefs",
    "meeting_cursor",
    "meeting_deeper",
    "meeting_delete",
    "meeting_detection",
    "meeting_detection_dismiss",
    "meeting_detection_set_event",
    "meeting_dismiss",
    "meeting_edit",
    "meeting_get",
    "meeting_list",
    "meeting_live",
    "meeting_pause",
    "meeting_permissions",
    "meeting_post_retry",
    "meeting_processes",
    "meeting_request",
    "meeting_resume",
    "meeting_segment_audio",
    "meeting_set_event",
    "meeting_set_speakers",
    "meeting_speaker_enroll",
    "meeting_speakers",
    "meeting_start",
    "meeting_status",
    "meeting_stop",
    "meeting_suggest_speakers",
    "meeting_summarize",
    "meeting_summary",
    "meeting_tap",
    "meeting_templates",
    "meeting_transcript"
  ],
  "topics": [
    "agent.events@1",
    "meeting.events@1"
  ]
} as const satisfies Requirements
