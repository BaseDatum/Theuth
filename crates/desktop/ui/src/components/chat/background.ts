import type { Api, TopicPayloads } from '@/rpc/bindings/client'
import { onTopic, type SessionsApi } from '@/store/sessions'

/** Replay events racing the read, including an unknown id firing before its snapshot arrives. */
export async function relistReminders(api: Pick<Api, 'list_reminders'>, store: SessionsApi, session: string, signal: AbortSignal) {
  const events: TopicPayloads['tools.reminders@1'][] = []
  const off = onTopic('tools.reminders@1', (e) => { if (e.session === session) events.push(e) })
  try {
    const rows = await api.list_reminders(session)
    if (signal.aborted) return
    store.getState().setReminders(session, rows)
    store.getState().applyEvents(events)
  } finally {
    off()
  }
}

/** Quiet PTYs remain in the Jobs card; the existing `ptys` set tracks screen activity only. */
export async function relistTerminals(api: Pick<Api, 'list_terminals'>, store: SessionsApi, session: string, signal: AbortSignal) {
  const events: TopicPayloads['tools.jobs@1'][] = []
  const off = onTopic('tools.jobs@1', (e) => {
    if (e.session === session && (e.type === 'JobStarted' || e.type === 'JobFinished')) events.push(e)
  })
  try {
    const rows = await api.list_terminals(session, false)
    if (signal.aborted) return
    const ids = new Set(rows.filter((r) => r.state === 'running').map((r) => r.id))
    for (const e of events) {
      if (e.type === 'JobStarted' && e.pty) ids.add(e.id)
      else if (e.type === 'JobFinished') ids.delete(e.id)
    }
    store.getState().setTerminals(session, ids)
  } finally {
    off()
  }
}
