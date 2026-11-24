// The owner's rule: dismissing or doing anything to an item in the Attention window never toasts. Success says nothing
// (the item leaves the list); a failure shows inline on the item (`ItemError`), not as a toast.
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { beforeEach, expect, test, vi } from 'vitest'
import { ATTENTION_FIXTURE } from '@/components/settings/attention-fixtures'
import { ATTENTION_KEY, useAttentionActions } from '@/components/hud/queries'
import type * as Rpc from '@/lib/rpc'
import type { AttentionItem } from '@/rpc/bindings/method.roster.output'
import { ItemError } from './ItemError'
import { setItemError, useAttentionBulk, useInboxActions, useReply } from './queries'

const toast = vi.hoisted(() => Object.assign(vi.fn(), { error: vi.fn(), message: vi.fn(), success: vi.fn() }))
vi.mock('sonner', () => ({ toast }))

const rpc = vi.hoisted(() => ({
  attention_dismiss: vi.fn(async (_id: string) => null as unknown),
  attention_snooze: vi.fn(async (_id: string, _until: number) => null as unknown),
  resolve_approval: vi.fn(async (_id: string, _d: 'allow' | 'deny') => true as boolean),
  handoff_resolve: vi.fn(async (_id: string, _o: string) => null as unknown),
  task_decide: vi.fn(async (_d: string, _a: string) => null as unknown),
  send_input: vi.fn(async (session: string) => session),
}))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => rpc }))

const F = ATTENTION_FIXTURE as AttentionItem[]
const ofKind = (kind: AttentionItem['kind']) => F.find((i) => i.kind === kind) as AttentionItem
const approval = F.find((i) => i.kind === 'approval' && i.approval) as AttentionItem

type Hooks = { bulk: ReturnType<typeof useAttentionBulk>; act: ReturnType<typeof useInboxActions>; loud: ReturnType<typeof useAttentionActions>; reply: ReturnType<typeof useReply> }

function setup(): { hooks: Hooks; qc: QueryClient; errorOf: (id: string) => string } {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false }, mutations: { retry: false } } })
  qc.setQueryData(ATTENTION_KEY, F)
  const hooks = {} as Hooks
  function Probe() {
    hooks.bulk = useAttentionBulk()
    hooks.act = useInboxActions()
    hooks.loud = useAttentionActions()
    hooks.reply = useReply()
    return null
  }
  const wrap = (node: ReactNode) => renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: node }))
  wrap(createElement(Probe))
  return { hooks, qc, errorOf: (id) => wrap(createElement(ItemError, { ids: [id] })) }
}

const noToast = () => {
  expect(toast).not.toHaveBeenCalled()
  expect(toast.error).not.toHaveBeenCalled()
  expect(toast.message).not.toHaveBeenCalled()
  expect(toast.success).not.toHaveBeenCalled()
}

beforeEach(() => {
  vi.clearAllMocks()
  setItemError(F.map((i) => i.id), null)
})

test('dismiss, snooze, allow / deny, a hand-off, a decision and a reply from the Attention window: no toast', async () => {
  const { hooks } = setup()
  const [a, b] = F
  await hooks.bulk.dismiss([a!.id])
  await hooks.bulk.snooze([b!.id], Date.now() + 3_600_000)
  await hooks.bulk.resolve([approval.approval!], 'allow')
  await hooks.act.approve.mutateAsync({ id: approval.approval!, allow: false })
  // "Already answered" was a toast.message; in the window it says nothing (the item leaves either way).
  rpc.resolve_approval.mockResolvedValueOnce(false)
  await hooks.act.approve.mutateAsync({ id: approval.approval!, allow: true })
  const handoff = ofKind('handoff')
  if (handoff) await hooks.act.handoff.mutateAsync({ id: handoff.id.replace(/^handoff:/, ''), outcome: 'done' })
  const decision = ofKind('decision')
  await hooks.act.decide.mutateAsync({ decision: decision.decision!, answer: 'ship it' })
  const finished = ofKind('finished')
  await hooks.reply.send(finished.session!, 'go on')
  expect(rpc.attention_dismiss).toHaveBeenCalledWith(a!.id)
  expect(rpc.task_decide).toHaveBeenCalled()
  noToast()
})

test('failures from the Attention window: no toast, the error shows inline on the item', async () => {
  const { hooks, errorOf } = setup()
  const [a] = F
  rpc.attention_dismiss.mockRejectedValueOnce(new Error('engine said no'))
  const outs = await hooks.bulk.dismiss([a!.id])
  expect(outs[0]?.resolution).toBe('failed')
  expect(errorOf(a!.id)).toContain('engine said no')
  expect(errorOf(a!.id)).toContain('role="alert"')

  rpc.resolve_approval.mockRejectedValueOnce(new Error('approval rpc down'))
  await expect(hooks.act.approve.mutateAsync({ id: approval.approval!, allow: true })).rejects.toThrow()
  expect(errorOf(approval.id)).toContain('approval rpc down')

  const decision = ofKind('decision')
  rpc.task_decide.mockRejectedValueOnce(new Error('decide failed'))
  await expect(hooks.act.decide.mutateAsync({ decision: decision.decision!, answer: 'x' })).rejects.toThrow()
  expect(errorOf(decision.id)).toContain('decide failed')

  const finished = ofKind('finished')
  rpc.send_input.mockRejectedValueOnce(new Error('session gone'))
  await expect(hooks.reply.send(finished.session!, 'hi')).rejects.toThrow()
  expect(errorOf(finished.id)).toContain('session gone')
  noToast()

  // A later success on the item clears its error.
  await hooks.bulk.dismiss([a!.id])
  expect(errorOf(a!.id)).toBe('')
})

test('outside the Attention window (no `report`) a failed action still toasts', async () => {
  const { hooks } = setup()
  rpc.resolve_approval.mockRejectedValueOnce(new Error('boom'))
  await expect(hooks.loud.approve.mutateAsync({ id: approval.approval!, allow: true })).rejects.toThrow()
  expect(toast.error).toHaveBeenCalledWith('boom')
})
