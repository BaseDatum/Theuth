import { useQuery } from '@tanstack/react-query'
import { createFileRoute } from '@tanstack/react-router'
import { useEffect, useState } from 'react'
import { Composer, type DraftCreate } from '@/components/chat/Composer'
import { Header } from '@/components/chat/Header'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
import { usePrivate } from '@/lib/private'
import { queryKeys, useApi } from '@/lib/rpc'

export const Route = createFileRoute('/')({ component: Index })

/** New chat: nothing exists until the first `send_input(null, …)` / `create_session` reply names the session. */
function Index() {
  const [effort, setEffort] = useState<string | null>()
  const [priv, setPriv] = useState(false)
  const [project, setProject] = useState('')
  useEffect(() => {
    document.title = ''
  }, [])
  const label = project.trim()
  const draft: DraftCreate = { private: priv, project: label ? label : null }
  return (
    <>
      <Header session={null} draftEffort={effort} onDraftEffort={setEffort} />
      <div className="flex min-h-0 flex-1 flex-col justify-center pb-24">
        <h1 className="mb-6 text-center text-[28px] leading-9 font-semibold">Where should we begin?</h1>
        <CreateOptions priv={priv} onPriv={setPriv} project={project} onProject={setProject} />
        <Composer session={null} draftEffort={effort} draftCreate={draft} />
      </div>
    </>
  )
}

/** Private toggle, project picker (existing labels + free text) and the retention warning with its acknowledgement (51 §1, §8.3). */
function CreateOptions({
  priv,
  onPriv,
  project,
  onProject,
}: {
  priv: boolean
  onPriv(v: boolean): void
  project: string
  onProject(v: string): void
}) {
  const api = useApi()
  const { data: sessions } = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() })
  const projects = [...new Set(sessions?.flatMap((s) => (s.project ? [s.project] : [])))].sort()
  const acknowledged = usePrivate((s) => s.acknowledged)
  const acknowledge = usePrivate((s) => s.acknowledge)
  return (
    <div className="mx-auto mb-3 w-full max-w-[768px] px-4 text-sm">
      <div className="flex flex-wrap items-center gap-x-6 gap-y-2">
        <label className="flex items-center gap-2">
          <Switch checked={priv} onCheckedChange={onPriv} aria-label="Private session" />
          <span>Private</span>
        </label>
        <div className="flex items-center gap-2">
          <Label htmlFor="new-chat-project" className="text-fg-secondary">
            Project
          </Label>
          <Input
            id="new-chat-project"
            list="new-chat-projects"
            className="h-8 w-48"
            placeholder="None"
            maxLength={128}
            value={project}
            onChange={(e) => onProject(e.target.value)}
          />
          <datalist id="new-chat-projects">
            {projects.map((p) => (
              <option key={p} value={p} />
            ))}
          </datalist>
        </div>
      </div>
      {priv && (
        <div className="mt-3 rounded-xl border border-amber-500/40 bg-amber-500/10 p-3">
          <p className="font-medium">Private session</p>
          <ul className="mt-1 list-disc space-y-0.5 pl-5 text-fg-secondary">
            <li>wth keeps this conversation, its artifacts and cost records in memory only; leaving or closing it deletes them.</li>
            <li>Scratch files it writes are real files in a temporary directory, removed on close (best effort: a crash or snapshot may keep them).</li>
            <li>This is not a sandbox: what a tool explicitly writes, sends or runs outside that directory is not undone.</li>
            <li>Model providers still see the request; their retention is not controlled by wth.</li>
            <li>A reload or a lost connection longer than 30 s closes the session.</li>
          </ul>
          <label className="mt-2 flex items-center gap-2">
            <input
              type="checkbox"
              className="size-4"
              checked={acknowledged}
              disabled={acknowledged}
              onChange={(e) => e.target.checked && acknowledge()}
            />
            <span>I understand</span>
          </label>
        </div>
      )}
    </div>
  )
}
