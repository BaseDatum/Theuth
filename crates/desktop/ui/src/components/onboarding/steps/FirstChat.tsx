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

/** The side panel: what the card under the pointer or focus produces, and the exact prompt it sends. */
function Preview({ starter: s }: { starter: Starter }) {
  const Icon = ICON[s.id]
  const prompt = starterPrompt(s.id, s.id === 'tour' ? MACHINE : s.id === 'research' ? 'your topic' : s.id === 'project' ? 'your folder' : '')
  return (
    <div className="flex flex-col gap-5">
      <span className="flex size-10 items-center justify-center rounded-lg bg-layer-1 text-fg-accent">
        <Icon className="size-5" />
      </span>
      <p className="text-[17px] leading-snug font-medium text-foreground">What you get</p>
      <ul className="flex flex-col gap-3">
        {s.yields.map((y, i) => (
          <li key={y} className="onb-rise flex gap-2.5 text-[13px] leading-relaxed text-fg-secondary" style={{ animationDelay: `${100 + 70 * i}ms` }}>
            <span className="mt-[7px] size-1.5 shrink-0 rounded-full bg-fg-accent" />
            {y}
          </li>
        ))}
      </ul>
      <div className="onb-rise flex flex-col gap-1.5 border-t border-border-subtle pt-4" style={{ animationDelay: '340ms' }}>
        <span className="text-[11px] font-medium tracking-[0.08em] text-fg-tertiary uppercase">The prompt it sends</span>
        <p className="line-clamp-6 text-[12px] leading-relaxed text-fg-tertiary">{prompt}</p>
      </div>
    </div>
  )
}

const ICON: Record<StarterId, typeof HardDrive> = { tour: HardDrive, page: LayoutDashboard, research: Telescope, project: FolderTree }
/** `This Mac` → `Mac`: the tour card names the machine it runs on. */
const MACHINE = LOCAL_NAME.replace(/^This /, '')

/** Step 3 of 3: one line on what chat is, then four prompts that show the reach, each sent at once; or the plain composer. */
export function FirstChat({ onStart, onOwn, onBack }: { onStart(text: string): Promise<void>; onOwn(): void; onBack(): void }) {
  const canSend = useQuerySupport(['new_session', 'send_input'])
  const ready = (useModels().data?.length ?? 0) > 0
  const [busy, setBusy] = useState<StarterId | null>(null)
  const [error, setError] = useState<{ message: string; id: StarterId; value?: string } | null>(null)
  const send = async (id: StarterId, value?: string) => {
    setBusy(id)
    setError(null)
    try {
      await onStart(starterPrompt(id, id === 'tour' ? MACHINE : value))
    } catch (e) {
      setError({ message: e instanceof Error ? e.message : String(e), id, value })
      setBusy(null)
    }
  }
  const disabled = !canSend || !ready || busy !== null
  const [preview, setPreview] = useState<StarterId>('tour')
  const shown = STARTERS.find((s) => s.id === preview) ?? STARTERS[0]!
  return (
    <StepFrame
      eyebrow="Step 3 of 3"
      title="Your first chat"
      description="Ask in plain words. theuth plans, works on this computer and shows each step as it goes. Pick one to watch it work:"
      aside={
        <div key={shown.id}>
          <Preview starter={shown} />
        </div>
      }
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
          <div key={s.id} className="flex" onMouseEnter={() => setPreview(s.id)} onFocus={() => setPreview(s.id)}>
            <StarterCard starter={s} busy={busy === s.id} disabled={disabled} onSend={(v) => void send(s.id, v)} />
          </div>
        ))}
      </div>
      {error && (
        <div role="alert" className="mt-4 flex max-w-3xl items-center gap-3 rounded-xl border border-danger-border bg-danger-bg px-4 py-3 text-[13px] text-danger">
          <span className="flex-1">Could not start the chat: {error.message}</span>
          {/* The retry sends into the chat the failed attempt created, never a second one. */}
          <Button variant="outline" size="sm" disabled={busy !== null} onClick={() => void send(error.id, error.value)}>
            Try again
          </Button>
        </div>
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
      <button type="button" disabled={disabled} onClick={() => onSend()} className={`${CARD} group flex w-full flex-col gap-3 p-4 disabled:pointer-events-none disabled:opacity-60`}>
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
      className={`${CARD} flex w-full flex-col gap-3 p-4 hover:bg-layer-2 focus-within:border-border-strong`}
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
