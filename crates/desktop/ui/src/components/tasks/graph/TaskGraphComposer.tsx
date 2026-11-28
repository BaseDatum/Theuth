// Floating bottom-right glass chat composer for the Task Graph, sharing `FloatingComposerDock` with Whiteboards.
// Lets the user ask an agent to add, link, or reorganize tasks directly on the graph; creates project-scoped
// sessions on first send and hands over compact graph context via `task_graph_send_context`.

import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { useCallback, useMemo, useState } from 'react'
import { FloatingComposerDock } from '@/components/chat/floating/FloatingComposerDock'
import { useSessionTitle } from '@/components/chat/floating/SessionChip'
import { useLoadedSession, useTurnReply } from '@/components/chat/floating/useTurnReply'
import { useDrafts } from '@/lib/drafts'
import { queryKeys, useApi } from '@/lib/rpc'
import type { Task } from '@/rpc/bindings/method.roster.output'
import { useQuerySupport } from '@/rpc/capabilities'
import { supports } from '@/rpc/discovery'
import { useConnection } from '@/store/connection'
import { useSessionField, useSessions } from '@/store/sessions'
import { useInvalidateTasks } from '../queries'
import {
  buildTaskGraphContext,
  graphScopeKey,
  MAX_GRAPH_CONTEXT_TASKS,
  partitionGraphSessions,
  resolveTargetSession,
  sendTaskGraphMessage,
  type TaskGraphSendArgs,
  type TaskGraphSendDeps,
  taskGraphPlaceholder,
} from './composer-model'

export {
  buildTaskGraphContext,
  graphScopeKey,
  MAX_GRAPH_CONTEXT_TASKS,
  partitionGraphSessions,
  resolveTargetSession,
  sendTaskGraphMessage,
  taskGraphPlaceholder,
  type TaskGraphSendArgs,
  type TaskGraphSendDeps,
}

export type TaskGraphComposerProps = {
  project: string | null
  tasks: readonly Task[]
  selected: string | null
}

export function TaskGraphComposer({ project, tasks, selected }: TaskGraphComposerProps) {
  const api = useApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const invalidate = useInvalidateTasks()
  const canList = useQuerySupport(['list_sessions'])
  const sessionsQuery = useQuery({
    enabled: canList,
    queryKey: queryKeys.sessions,
    queryFn: () => api.list_sessions(),
    staleTime: 15_000,
  })
  const sessions = sessionsQuery.data ?? []

  const { bound, recent } = useMemo(() => partitionGraphSessions(sessions, project), [sessions, project])

  // Remember the user's explicit session pick (or explicit "New session" = null) per project scope;
  // default to the latest session in this project if one exists, otherwise "New session".
  const scopeKey = graphScopeKey(project)
  const [pickedByScope, setPickedByScope] = useState<Record<string, string | null>>({})
  const session = resolveTargetSession(pickedByScope, project, bound)
  const setTarget = useCallback(
    (next: string | null) => setPickedByScope((prev) => ({ ...prev, [scopeKey]: next })),
    [scopeKey],
  )

  useLoadedSession(session)
  const running = (useSessionField(session ?? '', 'running') ?? false) as boolean
  const title = useSessionTitle(session, [...bound, ...recent])

  const { reply, dismiss } = useTurnReply({
    session,
    running,
    onTurnEnd: () => {
      invalidate()
      void qc.invalidateQueries({ queryKey: queryKeys.sessions })
    },
  })

  const openSession = useCallback((s: string) => void navigate({ to: '/c/$session', params: { session: s } }), [navigate])

  const deliver = async (m: { text: string; files: string[]; clientId: string }): Promise<string> => {
    dismiss()
    const discovery = useConnection.getState().discovery
    const canSendContext =
      discovery.phase === 'legacy' || supports(discovery, { methods: ['task_graph_send_context'] })
    const canSetProject =
      discovery.phase === 'legacy' || supports(discovery, { methods: ['set_session_project'] })
    const existing = session ? sessions.find((s) => s.id === session) : undefined

    return sendTaskGraphMessage(
      {
        createSession: async (opts) => (await api.create_session(opts)).session,
        setSessionProject: canSetProject ? (s, p) => api.set_session_project(s, p) : undefined,
        sendGraphContext: canSendContext ? (s, ctx) => api.task_graph_send_context(s, ctx) : undefined,
        sendInput: (s, body, files, id) => api.send_input(s, body, files, null, id),
        steer: (s, body, id, files) => api.steer(s, body, id, files),
      },
      {
        session,
        existingProject: existing?.project ?? null,
        running,
        project,
        selected,
        tasks,
        text: m.text,
        files: m.files,
        clientId: m.clientId,
        pick: session ? undefined : useDrafts.getState().pick,
      },
    )
  }

  const onSent = (targetSession: string) => {
    setTarget(targetSession)
    if (targetSession && targetSession !== session) {
      useSessions.getState().ensure(targetSession)
      useDrafts.getState().resetPick()
    }
    void qc.invalidateQueries({ queryKey: queryKeys.sessions })
    invalidate()
  }

  return (
    <FloatingComposerDock
      session={session}
      draftScope={`tg:${project ?? 'all'}:`}
      sessionChip={{
        title,
        bound,
        boundLabel: 'In this project',
        recent,
        onPick: (s) => setTarget(s),
        onNew: () => setTarget(null),
        onOpen: openSession,
        canvasSelector: '.react-flow__pane',
      }}
      placeholder={taskGraphPlaceholder(project)}
      deliver={deliver}
      onSent={onSent}
      reply={
        reply
          ? {
              text: reply.text,
              onDismiss: dismiss,
              onOpenSession: () => openSession(reply.session),
            }
          : null
      }
      marker="task-graph"
      focusShortcut
    />
  )
}
