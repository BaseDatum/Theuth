// 208: the card shown over a hovered or keyboard-focused node — everything the 200 px card truncates. No `@xyflow/react`
// import (the canvas positions it).
import { EyeMark } from '@/components/chat/ObservedBar'
import { counts } from '@/components/chat/broadcast'
import { ago } from '@/components/sidebar/AgentTree'
import { HarnessGlyph, shortModel } from '@/components/sidebar/harnessGlyph'
import type { GraphNode } from '@/rpc/bindings/method.agent.output'
import { KIND, MARK, STATUS, statusOf } from './encoding'
import { StatusChip } from './NodeBody'
import { nodeLabel, statusText } from './selectors'

const WAIT_WORD = { user: 'you', job: 'a job', children: 'its subagents' } as const

export function HoverCard({ node, attention, now, expiresAt }: { node: GraphNode; attention: number; now: number; expiresAt: number | null }) {
  const st = STATUS[statusOf(node)]
  const at = (ms: number) => ago(new Date(ms).toISOString(), now)
  const rows: [string, string][] = []
  if (node.model) rows.push(['Model', shortModel(node.model) || node.model])
  if (node.agent) rows.push(['Harness', node.agent])
  if (node.name && node.name !== node.title) rows.push(['Task', node.title])
  const work = counts(node.activity)
  if (work) rows.push(['Running', work])
  if (node.waits.length > 0) rows.push(['Waiting on', node.waits.map((w) => WAIT_WORD[w]).join(', ')])
  rows.push(['Started', `${at(node.started_ms)} ago`])
  rows.push(['Last event', `${at(node.last_event_ms)} ago`])
  if (expiresAt !== null) rows.push(['Fades', expiresAt > now ? `in ${ago(new Date(now).toISOString(), expiresAt)}` : 'now'])
  if (node.remote) rows.push(['Engine', `remote · ${node.owner}`])
  return (
    <div className="pointer-events-none w-72 rounded-xl border border-border-subtle bg-elevated p-3 text-xs text-foreground shadow-[0_8px_28px_-8px_rgb(0_0_0/0.35)]" data-hover-card={node.id}>
      <div className="flex items-start gap-2">
        <StatusChip glyph={st.glyph} tone={st.tone} className="mt-px size-5 p-1" />
        <div className="min-w-0 flex-1">
          <p className="text-sm leading-snug font-semibold break-words">{nodeLabel(node)}</p>
          <p className="mt-0.5 flex items-center gap-1.5 text-fg-tertiary">
            {node.agent && <HarnessGlyph agent={node.agent} className="size-3 shrink-0" />}
            {node.observed && <EyeMark />}
            <span>{KIND[node.kind].label}</span>
          </p>
        </div>
      </div>
      <p className="mt-2 font-medium" style={{ color: st.tone }}>
        {st.label}
        <span className="font-normal text-fg-secondary"> · {statusText(node)}</span>
      </p>
      {attention > 0 && (
        <p className="mt-1.5 flex items-center gap-1.5 font-medium text-danger">
          <span className="rounded-full bg-danger px-1.5 text-[10px] leading-4 text-white tabular-nums">{attention}</span>
          {attention === 1 ? 'item' : 'items'} in your inbox
          <span className="sr-only">{MARK.ring.detail}</span>
        </p>
      )}
      <dl className="mt-2 grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 border-t border-border-subtle pt-2">
        {rows.map(([k, v]) => (
          <div key={k} className="contents">
            <dt className="text-fg-tertiary">{k}</dt>
            <dd className="min-w-0 truncate text-fg-secondary tabular-nums">{v}</dd>
          </div>
        ))}
      </dl>
    </div>
  )
}
