// 158 §9.2 (W3): the project on meetings. A project is a free label (§9.1): the pickers offer the labels the
// sessions carry plus the styled ones, and accept a typed new one. The start form sends it in `StartMeeting.project`
// (`null` = the engine's remembered / `meeting.project` default), the meeting page changes it with
// `meeting_set_project`, and the list filters with `meeting_list(limit, project)`.

import type { QueryClient } from '@tanstack/react-query'
import { useProjects } from '@/components/chat/ProjectPicker'
import { useProjectStyles } from '@/lib/project-style'
import { queryKeys } from '@/lib/rpc'
import type { Api } from '@/rpc/api'
import type { Manifest } from '@/rpc/bindings/Manifest'
import type { MeetingId } from '@/rpc/bindings/MeetingId'
import type { MeetingSummary } from '@/rpc/bindings/MeetingSummary'

/** Distinct, trimmed, non-empty labels from every source, sorted. */
export function projectChoices(...sources: readonly (readonly (string | null | undefined)[])[]): string[] {
  const set = new Set<string>()
  for (const list of sources) for (const p of list) if (p?.trim()) set.add(p.trim())
  return [...set].sort()
}

/** `typed` names a project none of `projects` already is (case-insensitive); `''` when blank or known. */
export function newProjectName(typed: string, projects: readonly string[]): string {
  const name = typed.trim()
  return name !== '' && !projects.some((p) => p.toLowerCase() === name.toLowerCase()) ? name : ''
}

/** The labels the meeting pickers list: the sessions' projects (51 §3) plus the styled ones. */
export function useProjectChoices(): string[] {
  return projectChoices(useProjects(), Object.keys(useProjectStyles()))
}

/**
 * The meetings list, optionally one project's. Its key stays under the `queryKeys.meetings` prefix so the bus
 * invalidations (`Started`, `State`, `Deleted`, `Summarized`) reach the filtered list too; `null` is the plain list
 * every other reader shares.
 */
export function meetingsQuery(api: Pick<Api, 'meeting_list'>, project: string | null): { queryKey: readonly unknown[]; queryFn: () => Promise<MeetingSummary[]> } {
  return {
    queryKey: project === null ? queryKeys.meetings : [...queryKeys.meetings, { project }],
    queryFn: () => api.meeting_list(null, project),
  }
}

/**
 * The header chip's change: `meeting_set_project(id, project)` (`null` clears), and the returned manifest replaces
 * the cached one so the chip flips at once; the caller invalidates the lists and the sessions (the engine re-projects
 * the meeting's chat, §9.2 W-c).
 */
export async function changeProject(api: Pick<Api, 'meeting_set_project'>, qc: QueryClient, meeting: MeetingId, project: string | null): Promise<Manifest> {
  const manifest = await api.meeting_set_project(meeting, project)
  qc.setQueryData(queryKeys.meeting(meeting), manifest)
  return manifest
}
