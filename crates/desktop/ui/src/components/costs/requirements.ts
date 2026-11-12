import type { Requirements } from '@/rpc/discovery'

export const requirements = {
  "methods": [
    "cost_calls",
    "cost_cube",
    "put_attachment",
    "put_session_attachment",
    "usage_limits",
    "usage_limits_refresh"
  ],
  "topics": [
    "agent.cost-cube@1",
    "rpc.stream@1"
  ]
} as const satisfies Requirements
