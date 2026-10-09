import { open as pickFolder } from '@tauri-apps/plugin-dialog'
import { FolderOpen, MessageSquarePlus } from 'lucide-react'
import { useId, useState } from 'react'
import { toast } from 'sonner'
import { errorText } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { useApi } from '@/lib/rpc'
import { isTauri } from '@/lib/tauri'
import { LiveSession } from '@/routes/c.$session'
import { startSetupChat } from './setup'

/**
 * 151 §2 — the setup chat (= Routines' `RoutineChat`): the intro box until a session exists, then the chat page's
 * `LiveSession` with `first={false}`. The intro holds Repository (path + folder pick on the desktop + recent repos)
 * and the optional goal; "Start setup chat" is the §2 step 3 first send.
 */
export function SetupChat({ session, repos, onSession }: { session: string | null; repos: string[]; onSession: (id: string) => void }) {
  if (session) return <LiveSession session={session} first={false} />
  return <Intro repos={repos} onSession={onSession} />
}

function Intro({ repos, onSession }: { repos: string[]; onSession: (id: string) => void }) {
  const api = useApi()
  const listId = useId()
  const [repo, setRepo] = useState(repos[0] ?? '')
  const [goal, setGoal] = useState('')
  const [busy, setBusy] = useState(false)
  const path = repo.trim()
  const send = async () => {
    if (!path || busy) return
    setBusy(true)
    try {
      await startSetupChat(api, path, goal, onSession)
    } catch (e) {
      toast.error(errorText(e))
      setBusy(false)
    }
  }
  const pick = async () => {
    const picked = await pickFolder({ directory: true, multiple: false, defaultPath: path || undefined })
    if (typeof picked === 'string') setRepo(picked)
  }
  return (
    <div className="m-auto flex w-full max-w-[440px] flex-col gap-3 p-4">
      <h2 className="text-base font-semibold">Set up a study with theuth</h2>
      <label className="flex flex-col gap-1 text-xs text-fg-secondary">
        Repository
        <div className="flex gap-1">
          <Input list={listId} value={repo} placeholder="/absolute/path/to/repo" className="h-8 font-mono text-base md:text-xs" autoFocus onChange={(e) => setRepo(e.target.value)} aria-label="Repository" />
          {isTauri() && (
            <Button variant="outline" size="icon-sm" aria-label="Choose folder" onClick={() => void pick()}>
              <FolderOpen />
            </Button>
          )}
        </div>
        <datalist id={listId}>
          {repos.map((r) => (
            <option key={r} value={r} />
          ))}
        </datalist>
      </label>
      <label className="flex flex-col gap-1 text-xs text-fg-secondary">
        <span>
          What should get better? <span className="text-fg-tertiary">(optional)</span>
        </span>
        <Input
          value={goal}
          placeholder="t should reach its first frame faster"
          className="h-8 text-base md:text-sm"
          onChange={(e) => setGoal(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') void send()
          }}
        />
      </label>
      <p className="text-xs text-fg-tertiary">Opens a chat in study-setup mode. theuth interviews you, writes the benchmark and measures it. Nothing runs on your repo until you press Start.</p>
      <div className="flex justify-end">
        <Button size="sm" disabled={!path || busy} onClick={() => void send()}>
          <MessageSquarePlus /> Start setup chat
        </Button>
      </div>
    </div>
  )
}
