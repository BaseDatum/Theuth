import { Link } from '@tanstack/react-router'
import { CircleAlert, Eye, UserPlus } from 'lucide-react'
import { awaitsMyReview, needsPerson, ownerLabel } from './helpers'
import { useAgents, useTasks } from './queries'

/**
 * "Does it need me" (67 §12): blocked tasks waiting on a decision or a person, reviews where you are the reviewer,
 * proposed agents. One dense row per item, one click each. Renders nothing when the queue is empty or the engine
 * cannot answer yet.
 */
export function Queue() {
  const tasks = useTasks({ owner: null, state: ['blocked', 'review'], engine: null, project: null })
  const agents = useAgents()
  const decisions = tasks.data?.filter(needsPerson) ?? []
  const reviews = tasks.data?.filter(awaitsMyReview) ?? []
  const proposed = agents.data?.filter((a) => a.status === 'proposed' || a.file.proposed_by) ?? []
  if (decisions.length + reviews.length + proposed.length === 0) return null
  return (
    <ul className="flex shrink-0 flex-col divide-y divide-border-subtle border-b border-border-subtle bg-layer-1 text-[13px]">
      {decisions.map((t) => (
        <Item key={t.id} to={t.id} icon={<CircleAlert className="size-3.5 text-danger" />} label={t.title} meta={`${ownerLabel(t.owner)} · ${t.blockers[0]}`} />
      ))}
      {reviews.map((t) => (
        <Item key={t.id} to={t.id} icon={<Eye className="size-3.5 text-warning" />} label={t.title} meta={`review · ${ownerLabel(t.owner)}`} />
      ))}
      {proposed.map((a) => (
        <li key={a.file.name}>
          <Link to="/agents/$name" params={{ name: a.file.name }} className="flex h-8 items-center gap-2 px-3 hover:bg-layer-2">
            <UserPlus className="size-3.5 text-primary" />
            <span className="min-w-0 flex-1 truncate">
              Approve <b>{a.file.title}</b>
            </span>
            <span className="truncate text-fg-tertiary">proposed by {a.file.proposed_by ?? '—'}</span>
          </Link>
        </li>
      ))}
    </ul>
  )
}

function Item({ to, icon, label, meta }: { to: string; icon: React.ReactNode; label: string; meta: string }) {
  return (
    <li>
      <Link to="/tasks/$id" params={{ id: to }} className="flex h-8 items-center gap-2 px-3 hover:bg-layer-2">
        {icon}
        <span className="min-w-0 flex-1 truncate">{label}</span>
        <span className="truncate text-fg-tertiary">{meta}</span>
      </Link>
    </li>
  )
}
