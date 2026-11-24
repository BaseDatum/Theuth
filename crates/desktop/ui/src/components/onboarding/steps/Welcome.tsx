import { ArrowRight, Inbox, Library, TerminalSquare } from 'lucide-react'
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
      back={
        <Button variant="ghost" onClick={onSkip} className="text-fg-secondary">
          Skip — I know theuth
        </Button>
      }
      primary={
        <>
          <span className="hidden text-[13px] text-fg-tertiary sm:inline">about 2 minutes</span>
          <Button size="lg" onClick={onNext} autoFocus>
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
