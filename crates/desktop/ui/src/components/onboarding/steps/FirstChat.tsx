import { open as pickFolder } from '@tauri-apps/plugin-dialog'
import { ArrowRight, ArrowUp, FolderOpen, FolderTree, HardDrive, LayoutDashboard, LoaderCircle, PenLine, Telescope } from 'lucide-react'
import { useState } from 'react'
import { useModels } from '@/components/agents/queries'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { isTauri } from '@/lib/tauri'
import { LOCAL_NAME } from '@/link/engines'
import { useQuerySupport } from '@/rpc/capabilities'
import { CARD, StepFrame } from './Frame'
import { STARTERS, type Starter, type StarterId, starterPrompt } from './content'

const ICON: Record<StarterId, typeof HardDrive> = { tour: HardDrive, page: LayoutDashboard, research: Telescope, project: FolderTree }
/** `This Mac` → `Mac`: the tour card names the machine it runs on. */
const MACHINE = LOCAL_NAME.replace(/^This /, '')

/** Step 4: one line on what chat is, then four prompts that show the reach, each sent at once; or the plain composer. */
export function FirstChat({ onStart, onOwn, onBack }: { onStart(text: string): Promise<void>; onOwn(): void; onBack(): void }) {
  const canSend = useQuerySupport(['new_session', 'send_input'])
  const ready = (useModels().data?.length ?? 0) > 0
  const [busy, setBusy] = useState<StarterId | null>(null)
  const [error, setError] = useState<string | null>(null)
  const send = async (id: StarterId, value?: string) => {
    setBusy(id)
    setError(null)
    try {
      await onStart(starterPrompt(id, id === 'tour' ? MACHINE : value))
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
      setBusy(null)
    }
  }
  const disabled = !canSend || !ready || busy !== null
  return (
    <StepFrame
      eyebrow="Step 3 of 3"
      title="Your first chat"
      description="Ask in plain words. theuth plans, works on this computer and shows each step as it goes. Pick one to watch it work:"
      back={
        <Button variant="ghost" onClick={onBack} className="text-fg-secondary">
          Back
        </Button>
      }
      primary={
        <Button variant="outline" size="lg" onClick={onOwn}>
          <PenLine /> Write my own
        </Button>
      }
    >
      {!ready && <p className="mb-5 max-w-3xl rounded-xl border border-warning-border bg-warning-bg px-4 py-3 text-sm text-foreground">Connect a model first: go back one step, or later in Settings › Models.</p>}
      <div className="grid max-w-3xl gap-3 sm:grid-cols-2">
        {STARTERS.map((s) => (
          <StarterCard key={s.id} starter={s} busy={busy === s.id} disabled={disabled} onSend={(v) => void send(s.id, v)} />
        ))}
      </div>
      {error && (
        <p role="alert" className="mt-4 text-[13px] text-danger">
          Could not start the chat: {error}
        </p>
      )}
    </StepFrame>
  )
}

function StarterCard({ starter: s, busy, disabled, onSend }: { starter: Starter; busy: boolean; disabled: boolean; onSend(value?: string): void }) {
  const [value, setValue] = useState('')
  const Icon = ICON[s.id]
  const title = s.id === 'tour' ? `Tour ${LOCAL_NAME.replace(/^This/, 'this')}` : s.title
  const head = (
    <>
      <span className="flex size-9 items-center justify-center rounded-lg bg-layer-1 text-fg-accent">
        {busy ? <LoaderCircle className="size-[18px] animate-spin motion-reduce:animate-none" aria-hidden /> : <Icon className="size-[18px]" aria-hidden />}
      </span>
      <span className="flex flex-col gap-0.5">
        <span className="text-[15px] font-medium text-foreground">{title}</span>
        <span className="text-[13px] leading-snug text-fg-secondary">{s.blurb}</span>
      </span>
    </>
  )
  if (!s.input)
    return (
      <button type="button" disabled={disabled} onClick={() => onSend()} className={`${CARD} group flex flex-col gap-3 p-4 disabled:pointer-events-none disabled:opacity-60`}>
        {head}
        <span className="mt-auto flex items-center gap-1 text-[13px] font-medium text-fg-accent">
          {busy ? 'Starting…' : 'Send'} <ArrowRight className="size-3.5 transition-transform group-hover:translate-x-0.5 motion-reduce:transition-none" aria-hidden />
        </span>
      </button>
    )
  const ok = value.trim().length > 0
  const folder = s.id === 'project' && isTauri()
  return (
    <form
      className={`${CARD} flex flex-col gap-3 p-4 hover:bg-layer-2 focus-within:border-border-strong`}
      aria-label={title}
      onSubmit={(e) => {
        e.preventDefault()
        if (ok && !disabled) onSend(value)
      }}
    >
      {head}
      <div className="mt-auto flex items-center gap-2">
        {folder && (
          <Button
            type="button"
            variant="outline"
            size="icon"
            aria-label="Choose a folder"
            disabled={disabled}
            onClick={async () => {
              const picked = await pickFolder({ directory: true, multiple: false })
              if (typeof picked === 'string') setValue(picked)
            }}
          >
            <FolderOpen />
          </Button>
        )}
        <Input value={value} onChange={(e) => setValue(e.target.value)} placeholder={s.input.placeholder} aria-label={s.input.label} spellCheck={s.id !== 'project'} className={s.id === 'project' ? 'font-mono text-[13px]' : undefined} disabled={disabled} />
        <Button type="submit" size="icon" aria-label={`Send: ${title}`} disabled={!ok || disabled}>
          {busy ? <LoaderCircle className="animate-spin motion-reduce:animate-none" /> : <ArrowUp />}
        </Button>
      </div>
    </form>
  )
}
