/**
 * 209 §4.10, D13: the card of a `browser_show_element` call. The app draws the chrome — a lock glyph, the page origin in
 * ASCII (`scheme://host[:port]`, an IDN as `xn--…`), the browser that holds the page — and, below the frame, **Submit to
 * <host>** and **Cancel**. The frame is the sandboxed inline-page frame of 120 (`theuth-page` in the shell, `srcdoc`
 * elsewhere; `sandbox="allow-scripts allow-forms"`, never `allow-same-origin`) with the document of
 * `lib/element-form.ts` `formDocument`: no network, no submit, no script but the app's bootstrap. Submit asks the frame
 * for the typed values and hands them straight to `answer_element_form`; nothing here keeps them. Once resolved the card
 * shows how it ended (from `ElementFormResolved`, else the tool's receipt) and never a value.
 */
import { Ban, CircleCheck, Clock, Lock, LockOpen, TriangleAlert } from 'lucide-react'
import { type RefObject, useEffect, useId, useMemo, useRef, useState } from 'react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import {
  type ElementFormRequested,
  MAX_FRAME,
  MIN_FRAME,
  type Outcome,
  answerElementForm,
  attachSession,
  cancelElementForm,
  collectValues,
  formDocument,
  originLabel,
  outcomeOfReceipt,
  readFrameMessage,
  sourceLabel,
  useCanAnswer,
  useElementForms,
} from '@/lib/element-form'
import { pageDrop, pagePut } from '@/lib/pages'
import { isTauri } from '@/lib/tauri'
import { cn } from '@/lib/utils'
import type { Entry } from '@/store/sessions'
import { ToolSpinner } from './Indicators'

type Tool = Extract<Entry, { kind: 'tool' }>

const clampFrame = (h: number) => Math.min(MAX_FRAME, Math.max(MIN_FRAME, Math.ceil(h)))

/** The window's `message` events, for `collectValues`. */
const listenWindow = (cb: (ev: { source: unknown; data: unknown }) => void): (() => void) => {
  const h = (ev: MessageEvent) => cb(ev)
  window.addEventListener('message', h)
  return () => window.removeEventListener('message', h)
}

/** What the card says once the form is over: no value, ever. */
export function outcomeText(outcome: Outcome, origin: string | null): string {
  const where = origin ? (originLabel(origin)?.text ?? origin) : null
  switch (outcome) {
    case 'submitted':
      return where ? `Submitted to ${where}` : 'Submitted'
    case 'cancelled':
      return 'Cancelled: nothing was sent'
    case 'stale':
      return 'The page changed before it could be filled: nothing was typed'
    case 'timeout':
      return 'Not answered in time: nothing was sent'
  }
}

const OUTCOME_ICON = { submitted: CircleCheck, cancelled: Ban, stale: TriangleAlert, timeout: Clock } as const

/** The resolved card's one line, with the receipt (labels and names only) beneath when the transcript has it. */
export function ElementFormOutcome({ outcome, origin, receipt }: { outcome: Outcome; origin: string | null; receipt?: string }) {
  const Icon = OUTCOME_ICON[outcome]
  return (
    <div className="flex min-w-0 flex-col gap-1" role="status">
      <div className="flex min-w-0 items-center gap-2">
        <Icon className={cn('size-4 shrink-0', outcome === 'submitted' ? 'text-success' : 'text-fg-secondary')} aria-hidden="true" />
        <span className="wrap-anywhere">{outcomeText(outcome, origin)}</span>
      </div>
      {receipt && <span className="font-mono text-xs wrap-anywhere text-fg-secondary">{receipt.split('\n')[0]}</span>}
    </div>
  )
}

/** The sandboxed frame with the form's document; `onSubmitRequest` is Enter (or a submit button) inside it. */
function FormFrame({
  form,
  frame,
  label,
  onSubmitRequest,
}: {
  form: ElementFormRequested
  frame: RefObject<HTMLIFrameElement | null>
  label: string
  onSubmitRequest: () => void
}) {
  const key = useId()
  const doc = useMemo(() => formDocument(form.html, form.fields), [form.html, form.fields])
  const [src, setSrc] = useState<{ url?: string; doc?: string } | null>(null)
  const [failed, setFailed] = useState(false)
  const [height, setHeight] = useState(() => clampFrame((form.height || 0) + 16))
  const request = useRef(onSubmitRequest)
  request.current = onSubmitRequest

  useEffect(() => {
    setSrc(null)
    setFailed(false)
    if (!isTauri()) {
      setSrc({ doc })
      return
    }
    // One page per mount: the same card may be on screen twice (a pop-out window); dropping one must not 404 the other.
    const call = `form-${form.id}${key}`
    let live = true
    pagePut(call, doc).then(
      (url) => live && setSrc({ url }),
      () => live && setFailed(true),
    )
    return () => {
      live = false
      pageDrop(call).catch(() => {})
    }
  }, [doc, form.id, key])

  useEffect(() => {
    const onMessage = (ev: MessageEvent) => {
      if (!frame.current?.contentWindow || ev.source !== frame.current.contentWindow) return
      const m = readFrameMessage(ev.data)
      if (m?.type === 'size') setHeight(clampFrame(m.height))
      else if (m?.type === 'submit-request') request.current()
    }
    window.addEventListener('message', onMessage)
    return () => window.removeEventListener('message', onMessage)
  }, [frame])

  if (failed) return <div className="flex h-20 items-center px-4 text-sm text-fg-secondary">Unable to show the form</div>
  if (!src) return <div style={{ height }} className="animate-pulse bg-layer-2" />
  return (
    <iframe
      ref={frame}
      sandbox="allow-scripts allow-forms"
      src={src.url}
      srcDoc={src.doc}
      title={label}
      className="block w-full border-0 bg-white"
      style={{ height }}
    />
  )
}

/** The card of a `browser_show_element` call (`MessageTool`). */
export function ElementFormCard({ tool, session, readOnly }: { tool: Tool; session: string; readOnly?: boolean }) {
  const id = useElementForms((s) => s.calls[tool.callId])
  const form = useElementForms((s) => (id ? s.pending[id] : undefined))
  const done = useElementForms((s) => (id ? s.outcomes[id] : undefined))
  const canAnswer = useCanAnswer()
  const frame = useRef<HTMLIFrameElement>(null)
  const [busy, setBusy] = useState<'submit' | 'cancel' | null>(null)
  /** An answer was taken (or the form is over): the frame goes, and with it what was typed. */
  const [sent, setSent] = useState(false)
  const result = tool.result
  const receipt = result && !result.isError ? result.output : undefined
  const outcome = done?.outcome ?? (receipt ? outcomeOfReceipt(receipt) : null)
  const running = !tool.interrupted && (tool.pending || !result)
  // A card waiting with no form in this window (attached late, or the session is another engine's): ask for its forms.
  const missing = running && !form && !done
  useEffect(() => {
    if (missing) attachSession(session)
  }, [missing, session])
  const origin = form ? originLabel(form.origin) : null
  const shell = 'my-2 min-w-0 overflow-hidden rounded-xl border border-border-subtle bg-layer-1 text-sm'

  if (outcome) {
    return (
      <div className={cn(shell, 'px-4 py-3')}>
        <ElementFormOutcome outcome={outcome} origin={done?.origin ?? null} receipt={receipt} />
      </div>
    )
  }
  if (!form || readOnly) {
    return (
      <div className={cn(shell, 'flex min-w-0 items-center gap-2 px-4 py-3')}>
        <Lock className="size-4 shrink-0 text-fg-secondary" aria-hidden="true" />
        <span className={cn('min-w-0 flex-1 wrap-anywhere', result?.isError ? 'text-danger' : 'text-fg-secondary')}>
          {result ? result.output.split('\n')[0] : readOnly && form ? 'A form is waiting for an answer' : 'Waiting for the form…'}
        </span>
        {running && <ToolSpinner />}
      </div>
    )
  }

  const host = origin?.host ?? 'this page'
  const title = form.title || 'Form'
  const submit = async () => {
    const win = frame.current?.contentWindow
    if (busy || sent || !win || !origin) return
    setBusy('submit')
    try {
      const taken = await answerElementForm(form.id, await collectValues(win, form.fields, listenWindow), form.session)
      setSent(true)
      if (!taken) toast.error('This form was already answered')
    } catch (e) {
      // The error is the transport's or the frame's, never a value.
      toast.error(`Could not submit: ${e instanceof Error ? e.message : String(e)}`)
    } finally {
      setBusy(null)
    }
  }
  const cancel = async () => {
    if (busy || sent) return
    setBusy('cancel')
    try {
      await cancelElementForm(form.id, form.session)
      setSent(true)
    } catch (e) {
      toast.error(`Could not cancel: ${e instanceof Error ? e.message : String(e)}`)
    } finally {
      setBusy(null)
    }
  }

  const label = `${title} from ${origin?.text ?? 'an unknown origin'}`
  return (
    <section className={shell} aria-label={label} data-element-form={form.id}>
      <header className="flex min-w-0 flex-col gap-0.5 border-b border-border-subtle px-4 py-2">
        <div className="flex min-w-0 items-center gap-2">
          {origin?.secure ? (
            <Lock className="size-4 shrink-0 text-success" aria-label="Encrypted connection" role="img" />
          ) : (
            <LockOpen className="size-4 shrink-0 text-danger" aria-label={origin ? 'Not encrypted' : 'Unknown origin'} role="img" />
          )}
          <span className="min-w-0 truncate font-mono text-sm font-medium" title={origin?.text}>
            {origin?.text ?? 'unknown origin'}
          </span>
        </div>
        <span className="truncate text-xs text-fg-secondary">
          {title} · in {sourceLabel(form.source)}
        </span>
      </header>
      {sent ? (
        <div className="flex items-center gap-2 px-4 py-3 text-fg-secondary">
          <ToolSpinner />
          <span>Waiting for {host}…</span>
        </div>
      ) : (
        <FormFrame form={form} frame={frame} label={label} onSubmitRequest={() => void submit()} />
      )}
      <footer className="flex flex-wrap items-center justify-end gap-2 border-t border-border-subtle px-4 py-3">
        <p className="mr-auto min-w-0 text-xs text-fg-secondary">
          {origin ? 'What you type goes only to this page; the agent never sees it.' : 'The page has no web origin; it cannot be submitted from here.'}
        </p>
        <Button variant="outline" size="sm" disabled={!canAnswer || !!busy || sent} onClick={() => void cancel()} aria-label="Cancel: nothing is sent">
          Cancel
        </Button>
        <Button size="sm" disabled={!canAnswer || !origin || !!busy || sent} onClick={() => void submit()} aria-label={`Submit to ${host}`}>
          {busy === 'submit' ? 'Submitting…' : `Submit to ${host}`}
        </Button>
      </footer>
    </section>
  )
}
