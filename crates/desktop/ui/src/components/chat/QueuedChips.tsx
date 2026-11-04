import { AppWindow, Sparkles, X } from 'lucide-react'
import { toast } from 'sonner'
import { parseAppInput } from '@/components/apps/lifecycle'
import { ChannelIcon } from '@/components/icons/channels'
import { originLabel, shownText } from '@/lib/channel-origin'
import { label as chordLabel } from '@/lib/chord'
import { useApi } from '@/lib/rpc'
import { classify } from '@/lib/injected'
import { touch } from '@/lib/platform'
import { useShortcuts } from '@/lib/shortcuts'
import type { QueuedInput } from '@/rpc/bindings/QueuedInput'
import { useSessionField } from '@/store/sessions'
import { Attachments } from './Attachments'

/** A queued input's chip: a skill injection shows the skills icon, its name and the words; an app input its app and message;
 * a channel steer (172) its channel icon (label on hover) and the shown text; anything else `◇ text`. */
function label(q: QueuedInput) {
  const { text, channel } = q
  if (channel)
    return (
      <>
        <span title={originLabel(channel)}>
          <ChannelIcon via={channel.via} className="mr-1 inline size-3.5 align-[-2px]" />
        </span>
        {shownText(q)}
      </>
    )
  const app = parseAppInput(text)
  if (app) {
    return (
      <>
        <AppWindow className="mr-1 inline size-3.5 align-[-2px]" />
        <span className="font-medium">{app.name}</span>
        {`  ${app.message}`}
      </>
    )
  }
  const skill = classify(text)?.[0]
  if (skill?.kind !== 'skill') return `◇ ${text}`
  return (
    <>
      <Sparkles className="mr-1 inline size-3.5 align-[-2px]" />
      <span className="font-medium">{skill.name}</span>
      {skill.words && `  ${skill.words}`}
    </>
  )
}

/** Steers waiting for the current tool batch: click edits (withdraws into the composer), × withdraws. The last chip
 * names the interrupt gesture that delivers them now (item 8). */
export function QueuedChips({ session, onEdit }: { session: string; onEdit(text: string): void }) {
  const api = useApi()
  const queued = useSessionField(session, 'queued')
  const interject = useShortcuts((s) => s.chords.interject)
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
        <div key={q.id} className="flex max-w-full flex-col items-start gap-1">
          {/* 171F4 N7: the steer's own attachments at once, as its bubble will show them (thumbnails kept small here). */}
          <Attachments files={q.files} className="[&_img]:max-h-16" />
          <span className="flex max-w-full items-center gap-1 rounded-full bg-layer-2 pl-3 pr-1 text-sm text-fg-secondary">
            <button
              type="button"
              className="truncate py-1 hover:text-foreground"
              title="Edit"
              onClick={async () => {
                const text = await withdraw(q.id)
                if (text !== null) onEdit(text)
              }}
            >
              {label(q)}
              {q.defer && <span className="ml-1 text-xs text-fg-tertiary">next turn</span>}
              {q === queued[queued.length - 1] && (
                <span className="ml-1 text-xs text-fg-tertiary">queued · {touch() ? 'hold Steer' : chordLabel(interject)} to interrupt</span>
              )}
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
        </div>
      ))}
    </div>
  )
}
