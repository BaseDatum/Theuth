// Pure logic for the Task Graph floating composer: compact context payload, session partitioning,
// per-project session targeting, placeholder text, and send orchestration (tested in node).

import type { FloatingSessionItem } from '@/components/chat/floating/SessionChip'
import type { TaskGraphSendContext } from '@/rpc/bindings/method.agent.input'
import type { SessionSummary } from '@/rpc/bindings/method.agent.output'
import type { Task } from '@/rpc/bindings/method.roster.output'
import type { CreateSessionOptions } from '@/rpc/bindings/method.sessionsync.input'

export const MAX_GRAPH_CONTEXT_TASKS = 80

const PLAIN_CREATE_OPTIONS: CreateSessionOptions = {
  retention: 'durable',
  project: null,
  notebook: null,
  view: null,
  cwd: null,
  model: null,
  effort: null,
  parent_session: null,
  parent_engine: null,
  harness: null,
}

/** Build the compact `TaskGraphSendContext` handed to the engine right before `send_input` / `steer`. */
export function buildTaskGraphContext(args: {
  project: string | null
  selected: string | null
  tasks: readonly Task[]
}): TaskGraphSendContext {
  return {
    project: args.project ?? null,
    selected: args.selected ?? null,
    tasks: args.tasks.slice(0, MAX_GRAPH_CONTEXT_TASKS).map((t) => ({
      id: t.id,
      title: t.title,
      state: t.state,
      parent: t.parent ?? null,
      blockers: [...t.blockers],
    })),
  }
}

/** Partition unarchived root non-observed sessions into project-bound and recent other lists, newest first. */
export function partitionGraphSessions(
  sessions: readonly SessionSummary[],
  project: string | null,
  recentLimit = 8,
): { bound: FloatingSessionItem[]; recent: FloatingSessionItem[] } {
  const live = sessions
    .filter((s) => !s.archived && !s.parent && s.kind !== 'observed')
    .sort((a, b) => b.updated_at.localeCompare(a.updated_at))
  const bound =
    project !== null
      ? live.filter((s) => s.project === project).map((s) => ({ session: s.id, title: s.title || s.name }))
      : []
  const boundIds = new Set(bound.map((b) => b.session))
  const recent = live
    .filter((s) => !boundIds.has(s.id))
    .slice(0, recentLimit)
    .map((s) => ({ session: s.id, title: s.title || s.name }))
  return { bound, recent }
}

/** Scope key for remembering the user's picked session per project filter (`null` = all projects). */
export const graphScopeKey = (project: string | null): string => project ?? '__all__'

/** Resolve the active session for `project`: explicit user pick (including explicit `null` = New session), else latest project session, else `null`. */
export function resolveTargetSession(
  pickedByScope: Readonly<Record<string, string | null>>,
  project: string | null,
  bound: readonly FloatingSessionItem[],
): string | null {
  const key = graphScopeKey(project)
  if (Object.hasOwn(pickedByScope, key)) return pickedByScope[key] ?? null
  return bound[0]?.session ?? null
}

/** Idle textarea placeholder for the Task Graph floating composer. */
export function taskGraphPlaceholder(project: string | null): string {
  return project ? `Ask an agent to add or organize tasks in ${project}…` : 'Ask an agent to add, link, or organize tasks…'
}

export type TaskGraphSendDeps = {
  createSession(opts: CreateSessionOptions): Promise<string>
  setSessionProject?: (session: string, project: string | null) => Promise<unknown>
  sendGraphContext?: (session: string, context: TaskGraphSendContext) => Promise<unknown>
  sendInput(session: string, text: string, files: string[], clientId: string): Promise<unknown>
  steer(session: string, text: string, clientId: string, files: string[]): Promise<unknown>
}

export type TaskGraphSendArgs = {
  session: string | null
  existingProject?: string | null
  running: boolean
  project: string | null
  selected: string | null
  tasks: readonly Task[]
  text: string
  files: string[]
  clientId: string
  pick?: { model: string | null; effort: string | null }
}

/**
 * Deliver a message from the Task Graph composer:
 * 1. Create a project-scoped session if none is selected, or best-effort tag an untagged existing session with `project`.
 * 2. Hand over ephemeral graph context (`task_graph_send_context`), degrading gracefully if unsupported.
 * 3. `steer` if running, else `send_input`.
 */
export async function sendTaskGraphMessage(deps: TaskGraphSendDeps, args: TaskGraphSendArgs): Promise<string> {
  let target = args.session
  let isNew = false
  if (target === null) {
    target = await deps.createSession({
      ...PLAIN_CREATE_OPTIONS,
      project: args.project ?? null,
      model: args.pick?.model ?? null,
      effort: args.pick?.effort ?? null,
    })
    isNew = true
  } else if (args.project !== null && !args.existingProject && deps.setSessionProject) {
    try {
      await deps.setSessionProject(target, args.project)
    } catch {
      // Best-effort tagging; never block sending the user's message.
    }
  }

  if (deps.sendGraphContext) {
    try {
      await deps.sendGraphContext(
        target,
        buildTaskGraphContext({ project: args.project, selected: args.selected, tasks: args.tasks }),
      )
    } catch {
      // Older engine without `task_graph_send_context`: degrade cleanly and still deliver the message.
    }
  }

  if (!isNew && args.running) {
    await deps.steer(target, args.text, args.clientId, args.files)
  } else {
    await deps.sendInput(target, args.text, args.files, args.clientId)
  }

  return target
}
