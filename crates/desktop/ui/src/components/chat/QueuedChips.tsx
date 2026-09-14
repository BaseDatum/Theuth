import { X } from 'lucide-react'
import { toast } from 'sonner'
import { useApi } from '@/lib/rpc'
import { classify } from '@/lib/injected'
import { useSessionField } from '@/store/sessions'

/** A queued input's chip text: a skill injection reads `◇ name  words`, anything else its text. */
function label(text: string): string {
  const cls = classify(text)
  const skill = cls?.[0]
  if (skill?.kind === 'skill') return skill.words ? `◇ ${skill.name}  ${skill.words}` : `◇ ${skill.name}`
  return `◇ ${text}`
}

/** Steers waiting for the current tool batch: click edits (withdraws into the composer), × withdraws. */
export function QueuedChips({ session, onEdit }: { session: string; onEdit(text: string): void }) {
  const api = useApi()
  const queued = useSessionField(session, 'queued')
  if (!queued?.length) return null
  const withdraw = async (id: string) => {
    try {
      return await api.withdraw(session, id)
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
      return null
    }
  }
  return (
    <div className="mb-2 flex flex-wrap gap-2">
      {queued.map((q) => (
        <span key={q.id} className="flex max-w-full items-center gap-1 rounded-full bg-layer-2 pl-3 pr-1 text-sm text-fg-secondary">
          <button
            type="button"
            className="truncate py-1 hover:text-foreground"
            title="Edit"
            onClick={async () => {
              const text = await withdraw(q.id)
              if (text !== null) onEdit(text)
            }}
          >
            {label(q.text)}
          </button>
          <button
            type="button"
            aria-label="Withdraw"
            className="rounded-full p-1 hover:bg-layer-3"
            onClick={() => void withdraw(q.id)}
          >
            <X className="size-3.5" />
          </button>
        </span>
      ))}
    </div>
  )
}
