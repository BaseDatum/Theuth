import type { Requirements } from '@/rpc/discovery'

/** 223: what the Projects list and a project's page need; an engine without the project methods hides the view. */
export const requirements = {
  methods: [
    'doc_list',
    'list_project_styles',
    'list_sessions',
    'project_create',
    'project_delete',
    'project_update',
    'projects_list',
    'set_project_style',
    'tasks_list',
  ],
  topics: [],
} as const satisfies Requirements
