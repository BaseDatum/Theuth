import { type UseNavigateResult, useNavigate } from '@tanstack/react-router'
import { useEffect, useId, useState } from 'react'
import { Composer, type DraftCreate } from '@/components/chat/Composer'
import { Header } from '@/components/chat/Header'
import { LinkBanner } from '@/components/link/LinkFailure'
import { ProjectPicker } from '@/components/chat/ProjectPicker'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
import { draftProject, usePanes } from '@/lib/panes'
import { usePrivate } from '@/lib/private'

/**
 * The new-chat body shared by the `/new` route and a draft pane: header, prompt, composer, create options.
 * Nothing exists until the first `send_input(null, …)` / `create_session` reply names the session.
 */
export function NewChat({
  preset,
  pane,
  first = true,
  onClose,
  onCreated,
}: {
  /** `?project=<label>` from the sidebar's "New chat in <project>" (51 §3); followed when it changes. */
  preset?: string
  pane?: string
  first?: boolean
  onClose?(): void
  onCreated?(session: string): void
}) {
  const [effort, setEffort] = useState<string | null>()
  const [priv, setPriv] = useState(false)
  const [project, setProject] = useState<string | null>(preset ?? null)
  // `/` stays mounted across `/` → `/?project=x`, so the preset must follow the URL, not just seed the state.
  useEffect(() => setProject(preset ?? null), [preset])
  const draft: DraftCreate = { private: priv, project: project?.trim() || null }
  return (
    <>
      <Header session={null} draftEffort={effort} onDraftEffort={setEffort} first={first} onClose={onClose} />
      <LinkBanner />
      <div className="flex min-h-0 flex-1 flex-col justify-center pb-24">
        <h1 className="mb-6 text-center text-[28px] leading-9 font-semibold">Where should we begin?</h1>
        <Composer session={null} pane={pane} draftEffort={effort} draftCreate={draft} onCreated={onCreated} />
        <CreateOptions priv={priv} onPriv={setPriv} project={project} onProject={setProject} />
      </div>
    </>
  )
}

/**
 * The sidebar's New chat, ⇧⌘O and the project header's "New chat in <project>" (25 §12.3): with the view split, the
 * focused pane becomes a draft and the URL stays put; a single pane goes to the whole-window `/new`.
 */
export function openNewChat(navigate: UseNavigateResult<string>, project?: string | null): void {
  const panes = usePanes.getState()
  const draft = panes.fresh(project)
  if (!draft) void navigate({ to: '/new', search: project ? { project } : {} })
  // A new draft's pane takes the caret on mount; a kept one has to be told.
  else if (draft === panes.focused) window.dispatchEvent(new Event('theuth:focus-composer'))
}

/**
 * A split pane holding a New chat opened from the sidebar (25 §12.3). `id` is the draft leaf's client id; the
 * URL stays on the previously focused chat, and the first send swaps the created session into this leaf.
 */
export function NewChatPane({ id, first }: { id: string; first: boolean }) {
  const navigate = useNavigate()
  const focused = usePanes((s) => s.focused === id)
  useEffect(() => {
    if (focused) document.title = ''
  }, [focused])
  // Fresh from a drop: the user types at once. A remount while another pane is focused must not steal the caret.
  useEffect(() => {
    if (usePanes.getState().focused === id) window.dispatchEvent(new Event('theuth:focus-composer'))
  }, [id])
  const close = () => {
    const to = usePanes.getState().close(id)
    if (to) void navigate({ to: '/c/$session', params: { session: to } })
  }
  const created = (session: string) => {
    usePanes.getState().place(id, session, 'center')
    void navigate({ to: '/c/$session', params: { session } })
  }
  return <NewChat preset={draftProject(id)} pane={id} first={first} onClose={close} onCreated={created} />
}

/** Private toggle, project picker and the retention warning with its acknowledgement (51 §1, §8.3); sits under the composer. */
function CreateOptions({
  priv,
  onPriv,
  project,
  onProject,
}: {
  priv: boolean
  onPriv(v: boolean): void
  project: string | null
  onProject(v: string | null): void
}) {
  const acknowledged = usePrivate((s) => s.acknowledged)
  const acknowledge = usePrivate((s) => s.acknowledge)
  // Two draft panes may be open at once; label ids must not collide.
  const pid = useId()
  return (
    <div className="mx-auto mt-1 w-full max-w-[768px] px-4 text-sm">
      <div className="flex flex-wrap items-center gap-x-6 gap-y-2">
        <label className="flex items-center gap-2">
          <Switch checked={priv} onCheckedChange={onPriv} aria-label="Private session" />
          <span>Private</span>
        </label>
        <div className="flex items-center gap-2">
          <Label htmlFor={pid} className="text-fg-secondary">
            Project
          </Label>
          <ProjectPicker id={pid} value={project} onChange={onProject} />
        </div>
      </div>
      {priv && (
        <div className="mt-3 rounded-xl border border-warning-border/40 bg-warning-bg p-3">
          <p className="font-medium">Private session</p>
          <ul className="mt-1 list-disc space-y-0.5 pl-5 text-fg-secondary">
            <li>theuth keeps this conversation, its artifacts and cost records in memory only; leaving or closing it deletes them.</li>
            <li>Scratch files it writes are real files in a temporary directory, removed on close (best effort: a crash or snapshot may keep them).</li>
            <li>This is not a sandbox: what a tool explicitly writes, sends or runs outside that directory is not undone.</li>
            <li>Model providers still see the request; their retention is not controlled by theuth.</li>
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
