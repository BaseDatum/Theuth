// 224: `cloudnodes.events@1` cache fold and ref-counted subscription. Pure of React/UI imports so `lib/rpc.ts`
// (`useInvalidateOn`) can subscribe globally without pulling in settings components or creating a cycle.

import type { QueryClient } from '@tanstack/react-query'
import { notify } from '@/lib/notify'
import { on } from '@/store/sessions'
import type { CloudEvent, CloudList, CloudNode, ProviderKind, Stage } from './types'

/** One `CloudNodeProgress` folded: the latest stage, estimate and lines of a node's long job. */
export type CloudProgress = { stage: Stage | null; pct: number | null; eta_secs: number | null; lines: string[] }
export const LINES_CAP = 200

export function foldProgress(prev: CloudProgress | undefined, e: { stage: Stage | null; line: string; pct: number | null; eta_secs: number | null }): CloudProgress {
  const lines = [...(prev?.lines ?? []), e.line]
  return {
    stage: e.stage ?? prev?.stage ?? null,
    pct: e.pct ?? prev?.pct ?? null,
    eta_secs: e.eta_secs ?? prev?.eta_secs ?? null,
    lines: lines.length > LINES_CAP ? lines.slice(lines.length - LINES_CAP) : lines,
  }
}

export const cloudKeys = {
  all: ['cloud'] as const,
  list: ['cloud', 'list'] as const,
  providers: ['cloud', 'providers'] as const,
  sizes: (provider: ProviderKind | null, region: string | null) => ['cloud', 'sizes', provider, region] as const,
  k8s: (context: string | null) => ['cloud', 'k8s', context] as const,
  logs: (name: string) => ['cloud', 'logs', name] as const,
  progress: (name: string) => ['cloud', 'progress', name] as const,
}

type Changed = Extract<CloudEvent, { type: 'CloudNodeChanged' }>
type Progress = Extract<CloudEvent, { type: 'CloudNodeProgress' }>
type Removed = Extract<CloudEvent, { type: 'CloudNodeRemoved' }>
type Credential = Extract<CloudEvent, { type: 'CloudCredentialChanged' }>

export function upsertNode(list: CloudList | undefined, node: CloudNode): CloudList {
  const nodes = list?.nodes ?? []
  return { errors: list?.errors ?? [], nodes: nodes.some((n) => n.name === node.name) ? nodes.map((n) => (n.name === node.name ? node : n)) : [...nodes, node] }
}

export function removeNode(list: CloudList | undefined, name: string): CloudList {
  return { errors: list?.errors ?? [], nodes: (list?.nodes ?? []).filter((n) => n.name !== name) }
}

/** Fold one `cloudnodes.events` event into the caches; a create that finishes or fails says so in the status bar. */
export function applyCloudEvent(qc: QueryClient, e: CloudEvent): void {
  switch (e.type) {
    case 'CloudNodeChanged': {
      const before = qc.getQueryData<CloudList>(cloudKeys.list)?.nodes.find((n) => n.name === e.node.name)
      // The event never carries a price (only `cloud_list` / `cloud_status` ask the provider): keep the row's.
      // Likewise, keep a known `peer_id` if a ready/busy state event omits it so live mesh status stays matched.
      const keepPeer = e.node.peer_id === null && before?.peer_id && (e.node.state === 'ready' || e.node.state === 'busy')
      const node =
        before && (e.node.monthly_usd === null || keepPeer)
          ? {
              ...e.node,
              monthly_usd: e.node.monthly_usd === null ? before.monthly_usd : e.node.monthly_usd,
              peer_id: keepPeer ? before.peer_id : e.node.peer_id,
            }
          : e.node
      qc.setQueryData<CloudList>(cloudKeys.list, (list) => upsertNode(list, node))
      if (before?.state === 'creating' && e.node.state === 'ready') notify.success(`${e.node.name} is ready`, { id: `cloud-${e.node.name}` })
      if (before?.state === 'creating' && e.node.state === 'failed') notify.error(`${e.node.name} could not be created`, { id: `cloud-${e.node.name}`, description: e.node.error ?? undefined })
      return
    }
    case 'CloudNodeRemoved':
      qc.setQueryData<CloudList>(cloudKeys.list, (list) => removeNode(list, e.name))
      qc.removeQueries({ queryKey: cloudKeys.progress(e.name) })
      return
    case 'CloudNodeProgress':
      qc.setQueryData<CloudProgress>(cloudKeys.progress(e.name), (p) => foldProgress(p, e))
      return
    case 'CloudCredentialChanged':
      void qc.invalidateQueries({ queryKey: cloudKeys.providers })
      return
  }
}

/** One subscription per QueryClient however many mounts ask for it (a progress line must fold once). */
const subscribed = new WeakMap<QueryClient, { count: number; off: () => void }>()

/**
 * Subscribe `qc` to `cloudnodes.events@1` and fold via `applyCloudEvent`. Ref-counted per `QueryClient` so
 * `useInvalidateOn()` (global) and `useCloudEvents()` (standalone views/tests) never double-apply.
 */
export function subscribeCloudEvents(qc: QueryClient): () => void {
  const held = subscribed.get(qc)
  if (held) held.count++
  else {
    const offs = [
      on('cloudnodes.events@1', 'CloudNodeChanged', (e: Changed) => applyCloudEvent(qc, e)),
      on('cloudnodes.events@1', 'CloudNodeProgress', (e: Progress) => applyCloudEvent(qc, e)),
      on('cloudnodes.events@1', 'CloudNodeRemoved', (e: Removed) => applyCloudEvent(qc, e)),
      on('cloudnodes.events@1', 'CloudCredentialChanged', (e: Credential) => applyCloudEvent(qc, e)),
    ]
    subscribed.set(qc, { count: 1, off: () => offs.forEach((off) => off()) })
  }
  let released = false
  return () => {
    if (released) return
    released = true
    const s = subscribed.get(qc)
    if (!s) return
    if (--s.count === 0) {
      s.off()
      subscribed.delete(qc)
    }
  }
}
