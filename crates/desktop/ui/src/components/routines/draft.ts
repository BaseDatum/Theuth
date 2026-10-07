import { FRESH } from '@/components/chat/slash'
import type { Api } from '@/rpc/api'
import type { ScheduleFile } from '@/rpc/bindings/ScheduleFile'
import { type Entry, useSessions } from '@/store/sessions'

/** 108 §8.1 `RoutineDraft`: the fields of `schedules/<name>.md` as `ScheduleArgs` names them; the card's `args.routines[i]`. */
export type RoutineDraft = {
  name: string
  sentence: string
  cron?: string | null
  every?: string | null
  at?: string | null
  prompt: string
  quiet?: boolean
  model?: string | null
  until?: string | null
  max_runs?: number | null
  run_timeout?: string | null
  cwd?: string | null
  approvals?: boolean | null
  agent?: string | null
  project?: string | null
  when?: string | null
  fire?: string | null
  for?: string | null
  cooldown?: string | null
  mode?: string | null
  memory?: string | null
  expires?: string | null
  timeout?: string | null
}

export type RoutineDraftArgs = { routines: RoutineDraft[] }

/** An empty `/routines/new` form: weekday 09:00 clock, enabled, approvals on. */
export const NEW: ScheduleFile = {
  name: '',
  cron: '0 9 * * 1-5',
  every: null,
  at: null,
  quiet: false,
  missed_run: false,
  model: null,
  enabled: true,
  until: null,
  max_runs: null,
  cwd: null,
  approvals: true,
  agent: null,
  project: null,
  when: null,
  fire: null,
  for: null,
  cooldown: null,
  mode: null,
  memory: null,
  expires: null,
  timeout: null,
  created: null,
  run_timeout: null,
  runtime: null,
  prompt: '',
}

/** §6.4: a card's draft as the form's state; the draft never says `enabled` / `missed` / `created`, so those keep `NEW`'s. */
export function draftToFile(d: RoutineDraft): ScheduleFile {
  return {
    ...NEW,
    name: d.name,
    cron: d.cron ?? null,
    every: d.every ?? null,
    at: d.at ?? null,
    quiet: d.quiet ?? false,
    model: d.model ?? null,
    until: d.until ?? null,
    max_runs: d.max_runs ?? null,
    run_timeout: d.run_timeout ?? null,
    cwd: d.cwd ?? null,
    approvals: d.approvals ?? true,
    agent: d.agent ?? null,
    project: d.project ?? null,
    when: d.when ?? null,
    fire: d.fire ?? null,
    for: d.for ?? null,
    cooldown: d.cooldown ?? null,
    mode: d.mode ?? null,
    memory: d.memory ?? null,
    expires: d.expires ?? null,
    timeout: d.timeout ?? null,
    prompt: d.prompt,
  }
}

/** A card's draft and the tool call it came from (§6.4). */
export type Draft = { draft: RoutineDraft; callId: string }

/** The newest `routine_draft` call with args in a session's entries (§6.4), or `null`. */
export function latestDraft(entries: readonly Entry[]): Draft | null {
  for (let i = entries.length - 1; i >= 0; i--) {
    const e = entries[i]
    if (e?.kind !== 'tool' || e.name !== 'routine_draft') continue
    const d = (e.args as Partial<RoutineDraftArgs> | undefined)?.routines?.[0]
    if (d) return { draft: d, callId: e.callId }
  }
  return null
}

/** A `ScheduleFile` back as the markdown the engine wrote (front matter + body), for Edit by chat (§6.6). */
export function fileText(f: ScheduleFile): string {
  const keys: (keyof ScheduleFile)[] = ['cron', 'every', 'at', 'when', 'fire', 'for', 'cooldown', 'mode', 'memory', 'timeout', 'until', 'max_runs', 'expires', 'quiet', 'approvals', 'agent', 'project', 'model', 'cwd', 'run_timeout', 'enabled', 'created']
  const lines = keys.flatMap((k) => {
    const v = f[k]
    if (v === null || v === '' || (k === 'quiet' && v === false) || (k === 'approvals' && v === true) || (k === 'enabled' && v === true)) return []
    return [`${k}: ${typeof v === 'string' && /[:#]/.test(v) ? JSON.stringify(v) : String(v)}`]
  })
  if (f.missed_run) lines.push('missed: run')
  return `---\n${lines.join('\n')}\n---\n\n${f.prompt}\n`
}

/** §6.6: the first send of an Edit-by-chat pane. */
export const editSeed = (name: string, text: string, words: string) =>
  `Edit the existing routine \`${name}\`. Current file:\n\n\`\`\`markdown\n${text}\n\`\`\`\n\nChange: ${words}`

/**
 * §6.2 — the first send, in this order: a fresh root, its title, the slice seeded with `FRESH`, the page told (it
 * writes `?chat=<id>` and mounts `LiveSession`), then `use_skill` with the user's words. Nothing is marked sent
 * locally: `TurnStarted` echoes the skill card.
 */
export async function startRoutineChat(
  api: Pick<Api, 'new_session' | 'rename_session' | 'use_skill'>,
  text: string,
  seed: { name: string; text: string } | undefined,
  onSession: (id: string) => void,
): Promise<string> {
  const id = await api.new_session()
  await api.rename_session(id, seed ? `Routine ${seed.name}` : 'New routine')
  useSessions.getState().loadHistory(id, FRESH, 'replace')
  onSession(id)
  await api.use_skill(id, 'routines', seed ? editSeed(seed.name, seed.text, text) : text)
  return id
}
