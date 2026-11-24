import { ArrowRight, FileCode2, Inbox, Library, ShieldCheck, TerminalSquare } from 'lucide-react'
import type { ReactNode } from 'react'
import { Button } from '@/components/ui/button'
import { StepFrame } from './Frame'

const PROPS = [
  { icon: TerminalSquare, title: 'Agents that act on your machine', text: 'They run commands, edit files and browse the web for you, and show every step.' },
  { icon: Library, title: 'Everything they make lands in your Library', text: 'Pages, pictures, research and notes, kept and searchable.' },
  { icon: Inbox, title: 'You approve what matters from your Inbox', text: 'Questions, approvals and finished work wait for you in one place.' },
] as const

/** Step 1: what theuth is in three lines, then setup or skip. */
export function Welcome({ onNext, onSkip }: { onNext(): void; onSkip(): void }) {
  return (
    <StepFrame
      eyebrow="Welcome to theuth"
      title="Agents that do the work, on your computer."
      description="A two-minute setup: connect a model, pick your default, and watch your first agent get something done."
      aside={<Story />}
      back={
        <Button variant="ghost" onClick={onSkip} className="text-fg-secondary">
          Skip — I know theuth
        </Button>
      }
      primary={
        <>
          <span className="hidden text-[13px] text-fg-tertiary sm:inline">about 2 minutes</span>
          <Button size="lg" onClick={onNext}>
            Get set up <ArrowRight />
          </Button>
        </>
      }
    >
      <ul className="flex max-w-xl flex-col gap-6">
        {PROPS.map(({ icon: Icon, title, text }) => (
          <li key={title} className="flex gap-4">
            <span className="flex size-10 shrink-0 items-center justify-center rounded-lg border border-border-subtle bg-layer-2 text-fg-accent">
              <Icon className="size-5" aria-hidden />
            </span>
            <span className="flex flex-col gap-0.5 pt-0.5">
              <span className="text-[15px] font-medium text-foreground">{title}</span>
              <span className="text-sm text-fg-secondary">{text}</span>
            </span>
          </li>
        ))}
      </ul>
    </StepFrame>
  )
}

/** One request told in three beats, as the app shows them: you ask, it lands in the Library, the Inbox asks you. */
function Story() {
  const beat = (i: number, node: ReactNode) => (
    <div className="onb-rise" style={{ animationDelay: `${150 + 260 * i}ms` }}>
      {node}
    </div>
  )
  const link = (i: number) => <div className="onb-rise ml-6 h-5 w-px bg-border-strong" style={{ animationDelay: `${280 + 260 * i}ms` }} />
  return (
    <div className="flex flex-col">
      {beat(
        0,
        <div className="ml-auto max-w-[85%] rounded-2xl rounded-br-md bg-layer-3 px-3.5 py-2.5 text-[13px] leading-snug text-foreground shadow-xs">Make me a calm start-of-day page</div>,
      )}
      {link(0)}
      {beat(
        1,
        <div className="flex items-center gap-3 rounded-xl border border-border-subtle bg-layer-1 p-3 shadow-xs">
          <span className="flex size-9 items-center justify-center rounded-lg bg-layer-2 text-fg-accent">
            <FileCode2 className="size-[18px]" />
          </span>
          <span className="flex min-w-0 flex-col">
            <span className="truncate text-[13px] font-medium text-foreground">start-of-day.html</span>
            <span className="text-[12px] text-fg-tertiary">Library · Artifacts</span>
          </span>
        </div>,
      )}
      {link(1)}
      {beat(
        2,
        <div className="flex flex-col gap-2.5 rounded-xl border border-border-subtle bg-layer-1 p-3 shadow-xs">
          <span className="flex items-center gap-2 text-[12px] text-fg-tertiary">
            <ShieldCheck className="size-3.5 text-fg-accent" /> Inbox · Approval
          </span>
          <span className="text-[13px] leading-snug text-foreground">Save it to ~/Sites and open it in your browser?</span>
          <span className="flex gap-1.5">
            <span className="rounded-md bg-primary px-2.5 py-1 text-[12px] font-medium text-primary-foreground">Allow</span>
            <span className="rounded-md border border-border-subtle px-2.5 py-1 text-[12px] text-fg-secondary">Deny</span>
          </span>
        </div>,
      )}
    </div>
  )
}
