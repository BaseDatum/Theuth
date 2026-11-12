// 177 U5: **Test summary call** — one `test_summary_call` on the model a summary would use, shown in Settings › Memory
// and in the Memory panel's failing block.

import { useMutation } from '@tanstack/react-query'
import { CircleCheck, LoaderCircle, TriangleAlert } from 'lucide-react'
import { toast } from 'sonner'
import { summaryTestStats } from '@/components/settings/providers'
import { Button } from '@/components/ui/button'
import { useApi } from '@/lib/rpc'
import type { SummaryTest } from "@/rpc/bindings/method.agent.output"

/** What one `test_summary_call` came back with — the line and its numbers, or the error and what to do about it. */
export function SummaryTestResult({ t }: { t: SummaryTest }) {
  return t.ok ? (
    <div role="status" className="flex flex-col gap-1 rounded-md border border-success-border bg-success-bg px-3 py-2 text-[13px]">
      <p className="flex items-center gap-1.5 font-medium text-success">
        <CircleCheck className="size-3.5 shrink-0" aria-hidden /> Summary call worked
      </p>
      {t.line && <p className="break-words">{t.line}</p>}
      <p className="text-fg-secondary">{`${t.model} · ${summaryTestStats(t)}`}</p>
    </div>
  ) : (
    <div role="status" className="flex flex-col gap-1 rounded-md border border-warning-border bg-warning-bg px-3 py-2 text-[13px]">
      <p className="flex items-center gap-1.5 font-medium text-warning">
        <TriangleAlert className="size-3.5 shrink-0" aria-hidden /> {t.error ?? 'The summary call failed'}
      </p>
      {t.hint && <p>{t.hint}</p>}
      <p className="text-fg-secondary">{`${t.model} · ${summaryTestStats(t)}`}</p>
    </div>
  )
}

/** The button and what is under it: disabled without a model or while a call runs ("Testing…" and a wait note), then the result. */
export function SummaryTestView({ model, pending, result, size = 'sm', onTest }: { model: string | null | undefined; pending: boolean; result: SummaryTest | undefined; size?: 'sm' | 'xs'; onTest(model: string): void }) {
  return (
    <div className="flex w-full flex-col items-start gap-2">
      <Button size={size} variant="outline" disabled={!model || pending} onClick={() => model && onTest(model)}>
        {pending && <LoaderCircle className="animate-spin" />}
        {pending ? 'Testing…' : 'Test summary call'}
      </Button>
      {pending ? <p className="text-[13px] text-fg-secondary">{`Waiting for ${model}; a local model can take tens of seconds.`}</p> : result && <SummaryTestResult t={result} />}
    </div>
  )
}

function Run({ model, size }: { model: string | null | undefined; size?: 'sm' | 'xs' }) {
  const api = useApi()
  const test = useMutation({ mutationFn: (m: string) => api.test_summary_call(m), onError: (e) => void toast.error(e.message) })
  return <SummaryTestView model={model} pending={test.isPending} result={test.data} size={size} onTest={(m) => test.mutate(m)} />
}

/** **Test summary call** on `model` (`null` = nothing to test). A new model starts clean: the last result belongs to the old one (R1-7). */
export function SummaryTestCall({ model, size }: { model: string | null | undefined; size?: 'sm' | 'xs' }) {
  return <Run key={model ?? ''} model={model} size={size} />
}
