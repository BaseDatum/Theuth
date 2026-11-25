import type { Requirements } from '@/rpc/discovery'
import { WB_METHODS } from './wire'

/** 218: what the whiteboard list, page and sidebar row need from the server. */
export const requirements = {
  methods: [
    'doc_delete',
    'doc_list',
    'doc_open',
    'doc_rename',
    'doc_update',
    'doc_subscribe',
    'send_input',
    'steer',
    ...WB_METHODS,
  ],
  topics: ['document.changed@1'],
} as const satisfies Requirements
