/**
 * 209 §4.10, D13: the card of a `browser_show_element` call. The app draws the chrome — a lock glyph, the page origin in
 * ASCII (`scheme://host[:port]`, an IDN as `xn--…`, its registrable end never truncated), the browser that holds the
 * page, where the form sends what is typed (a warning when that is another origin than the page's) — and, below the
 * frame, **Submit to <host>** and **Cancel**. The frame is the sandboxed inline-page frame of 120 (`theuth-page` in the
 * shell, `srcdoc` elsewhere; `sandbox="allow-scripts allow-forms"`, never `allow-same-origin`) with the document of
 * `lib/element-form.ts` `formDocument`: no network, no submit, no script but the app's bootstrap. Submit asks the frame
 * for the values the user changed and hands them straight to `answer_element_form` on the card's own engine; nothing
 * here keeps them. Once resolved the card shows how it ended (from `ElementFormResolved`, else the tool's receipt) and
 * never a value.
 */
import { Ban, CircleCheck, Clock, Lock, LockOpen, TriangleAlert, Users } from 'lucide-react'
import { type RefObject, useEffect, useId, useMemo, useRef, useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { engines } from '@/components/tasks/home'
import { useMesh } from '@/components/tasks/queries'
import { Button } from '@/components/ui/button'
import {
  type ElementFormRequested,
  type OriginLabel,
  MAX_FRAME,
  MIN_FRAME,
  answerElementForm,
  attachSession,
  cancelElementForm,
  collectValues,
  formDocument,
  markShown,
  originLabel,
  outcomeOfReceipt,
  readFrameMessage,
  receiptLine,
  reloadSession,
  sourceLabel,
  useCanAnswer,
} from '@/lib/element-form'
import { pageDrop, pagePut } from '@/lib/pages'
import { isTauri } from '@/lib/tauri'
import { cn } from '@/lib/utils'
import { useConnection } from '@/store/connection'
import { type FormOutcome, useElementForms } from '@/store/element-forms'
import type { Entry } from '@/store/sessions'
import { ToolSpinner } from './Indicators'

type Tool = Extract<Entry, { kind: 'tool' }>

const TOUCH = 'pointer-coarse:min-h-11'
/** After an answer was taken, how long the card waits for `ElementFormResolved` before it asks the engine again. */
export const SETTLE_MS = 30_000

const clampFrame = (h: number) => Math.min(MAX_FRAME, Math.max(MIN_FRAME, Math.ceil(h)))

/** The window's `message` events, for `collectValues`. */
const listenWindow = (cb: (ev: { source: unknown; data: unknown }) => void): (() => void) => {
  const h = (ev: MessageEvent) => cb(ev)
  window.addEventListener('message', h)
  return () => window.removeEventListener('message', h)
}

/** An origin on one line: the start truncates, the registrable end (`tail`) never does, so a phishing suffix shows. */
function OriginText({ label, className }: { label: OriginLabel; className?: string }) {
  return (
    <span className={cn('flex min-w-0 font-mono', className)} title={label.text}>
      <span className="truncate">{label.head}</span>
      <span className="shrink-0">{label.tail}</span>
    </span>
  )
}

/** What the card says once the form is over: no value, ever. */
export function outcomeText(outcome: FormOutcome, origin: string | null): string {
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
    case 'elsewhere':
      return 'Answered in another window'
  }
}

const OUTCOME_ICON = { submitted: CircleCheck, cancelled: Ban, stale: TriangleAlert, timeout: Clock, elsewhere: Users } as const

/** The resolved card's one line, with the receipt (labels and names only) beneath when the transcript has it; `focus`
 * takes the keyboard focus (the card was answered here, and the control that had it is gone). */
export function ElementFormOutcome({ outcome, origin, receipt, focus }: { outcome: FormOutcome; origin: string | null; receipt?: string; focus?: boolean }) {
  const Icon = OUTCOME_ICON[outcome]
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (focus) ref.current?.focus()
  }, [focus])
  return (
    <div ref={ref} tabIndex={-1} className="flex min-w-0 flex-col gap-1 outline-none" role="status">
      <div className="flex min-w-0 items-center gap-2">
        <Icon className={cn('size-4 shrink-0', outcome === 'submitted' ? 'text-success' : 'text-fg-secondary')} aria-hidden="true" />
        <span className="wrap-anywhere">{outcomeText(outcome, origin)}</span>
      </div>
      {receipt && <span className="font-mono text-xs wrap-anywhere text-fg-secondary">{receiptLine(receipt)}</span>}
    </div>
  )
}

/** The sandboxed frame with the form's document; `onSubmitRequest` is Enter (or a submit button) inside it, `onShown`
 * runs once the frame is on screen. Its content width is capped at the element's own (`form.width`). */
function FormFrame({
  form,
  frame,
  label,
  onSubmitRequest,
  onShown,
}: {
  form: ElementFormRequested
  frame: RefObject<HTMLIFrameElement | null>
  label: string
  onSubmitRequest: () => void
  onShown: () => void
}) {
  const key = useId()
  const doc = useMemo(() => formDocument(form.html, form.fields), [form.html, form.fields])
  const [src, setSrc] = useState<{ url?: string; doc?: string } | null>(null)
  const [failed, setFailed] = useState(false)
  const [height, setHeight] = useState(() => clampFrame((form.height || 0) + 16))
  const request = useRef(onSubmitRequest)
  request.current = onSubmitRequest
  const shown = useRef(onShown)
  shown.current = onShown

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
    if (src) shown.current()
  }, [src])

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
      style={{ height, maxWidth: form.width > 0 ? form.width + 16 : undefined }}
    />
  )
}

const SHELL = 'my-2 min-w-0 overflow-hidden rounded-xl border border-border-subtle bg-layer-1 text-sm'

/**
 * A pending form: chrome, frame, Submit and Cancel. `onAnswered` runs when this window's answer was taken. Cancel stays
 * live after Submit (the first answer wins, so it is harmless), and a card that waits `SETTLE_MS` after its answer asks
 * the engine for its session's cards again: a lost `ElementFormResolved` cannot leave it stuck.
 */
export function ElementFormPanel({ form, onAnswered }: { form: ElementFormRequested; onAnswered?: () => void }) {
  const api = useConnection((s) => s.api)
  const discovery = useConnection((s) => s.discovery)
  const canAnswer = useCanAnswer()
  const resolve = useElementForms((s) => s.resolve)
  const frame = useRef<HTMLIFrameElement>(null)
  const [busy, setBusy] = useState<'submit' | 'cancel' | null>(null)
  /** This window's answer was taken: the frame goes, and with it what was typed. */
  const [sent, setSent] = useState<'submit' | 'cancel' | null>(null)
  const [late, setLate] = useState(false)
  const waiting = useRef<HTMLDivElement>(null)
  const origin = originLabel(form.origin)
  const mesh = useMesh().data
  const action = originLabel(form.action_origin || form.origin)
  const crossOrigin = !!origin && !!action && action.text !== origin.text

  useEffect(() => {
    if (!sent) return
    waiting.current?.focus()
    const timer = setTimeout(() => {
      setLate(true)
      reloadSession(form.session)
    }, SETTLE_MS)
    return () => clearTimeout(timer)
  }, [sent, form.session])

  const host = action?.host ?? 'this page'
  const submit = async () => {
    const win = frame.current?.contentWindow
    if (busy || sent || !win || !action || !api) return
    setBusy('submit')
    try {
      const values = await collectValues(win, form.fields, listenWindow)
      if (Object.keys(values).length === 0) {
        toast.error('Nothing to submit: type into the form, or Cancel')
        return
      }
      if (await answerElementForm(api, form.id, values, form.session)) {
        setSent('submit')
        onAnswered?.()
      } else resolve(form.id, 'elsewhere')
    } catch (e) {
      // The error is the transport's or the frame's, never a value.
      toast.error(`Could not submit: ${e instanceof Error ? e.message : String(e)}`)
    } finally {
      setBusy(null)
    }
  }
  const cancel = async () => {
    if (busy || sent === 'cancel' || !api) return
    setBusy('cancel')
    try {
      if (await cancelElementForm(api, form.id, form.session)) {
        setSent('cancel')
        onAnswered?.()
      } else if (sent) toast('Already submitted: waiting for the page')
      else resolve(form.id, 'elsewhere')
    } catch (e) {
      toast.error(`Could not cancel: ${e instanceof Error ? e.message : String(e)}`)
    } finally {
      setBusy(null)
    }
  }

  const label = `Form from ${origin?.text ?? 'an unknown origin'}`
  return (
    <section className={SHELL} aria-label={label} data-element-form={form.id}>
      <header className="flex min-w-0 flex-col gap-0.5 border-b border-border-subtle px-4 py-2">
        <div className="flex min-w-0 items-center gap-2">
          {origin?.secure ? (
            <Lock className="size-4 shrink-0 text-success" aria-label="Encrypted connection" role="img" />
          ) : (
            <LockOpen className="size-4 shrink-0 text-danger" aria-label={origin ? 'Not encrypted' : 'Unknown origin'} role="img" />
          )}
          {origin ? <OriginText label={origin} className="text-sm font-medium" /> : <span className="font-mono font-medium">unknown origin</span>}
        </div>
        {/* The title is the model's (or the page's) word, not the app's: it is quoted and attributed. */}
        <span className="truncate text-xs text-fg-secondary">
          {form.title ? `the agent calls it “${form.title}” · ` : ''}in {sourceLabel(form.source, (id) => engines(mesh).find((e) => e.id === id)?.name)}
        </span>
      </header>
      {sent ? (
        <div ref={waiting} tabIndex={-1} role="status" className="flex items-center gap-2 px-4 py-3 text-fg-secondary outline-none">
          <ToolSpinner />
          <span className="wrap-anywhere">
            {sent === 'cancel' ? 'Cancelling…' : late ? `Sent. Still waiting for ${host}; the outcome will show here.` : `Waiting for ${host}…`}
          </span>
        </div>
      ) : (
        <FormFrame
          form={form}
          frame={frame}
          label={label}
          onSubmitRequest={() => void submit()}
          onShown={() => api && markShown(api, discovery, form)}
        />
      )}
      <footer className="flex flex-col gap-2 border-t border-border-subtle px-4 py-3">
        {action ? (
          <div className="flex min-w-0 items-center gap-1 text-xs text-fg-secondary">
            <span className="shrink-0">Sends to</span>
            <OriginText label={action} />
          </div>
        ) : (
          <p className="text-xs text-fg-secondary">The page has no web origin; it cannot be submitted from here.</p>
        )}
        {crossOrigin && (
          <p className="flex min-w-0 items-start gap-2 text-xs text-warning">
            <TriangleAlert className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
            <span className="wrap-anywhere">
              This form sends what you type to {action.text}, not to {origin.text}, the page you see.
            </span>
          </p>
        )}
        <div className="flex min-w-0 flex-wrap items-center justify-end gap-2">
          <p className="mr-auto min-w-0 text-xs text-fg-secondary">The agent never sees what you type.</p>
          <Button
            variant="outline"
            size="sm"
            className={TOUCH}
            disabled={!canAnswer || !!busy || sent === 'cancel'}
            onClick={() => void cancel()}
            aria-label="Cancel: nothing is sent"
          >
            Cancel
          </Button>
          <Button
            size="sm"
            className={cn(TOUCH, 'min-w-0 max-w-full shrink')}
            disabled={!canAnswer || !action || !!busy || !!sent}
            onClick={() => void submit()}
            aria-label={`Submit to ${host}`}
            title={`Submit to ${host}`}
          >
            <span className="truncate">{busy === 'submit' ? 'Submitting…' : `Submit to ${host}`}</span>
          </Button>
        </div>
      </footer>
    </section>
  )
}

/** The card of a `browser_show_element` call (`MessageTool`). */
export function ElementFormCard({ tool, session, readOnly }: { tool: Tool; session: string; readOnly?: boolean }) {
  const id = useElementForms((s) => s.calls[tool.callId])
  const form = useElementForms((s) => (id ? s.pending[id] : undefined))
  const done = useElementForms((s) => (id ? s.outcomes[id] : undefined))
  /** This window answered it: the outcome line takes the focus when it comes. */
  const [answered, setAnswered] = useState(false)
  const result = tool.result
  // The receipt, or the stale error (`isError`): both name the outcome on their first line (§4.10).
  const receipt = result?.output
  const fromReceipt = receipt ? outcomeOfReceipt(receipt) : null
  // The receipt is final; `elsewhere` only stands in until the engine's outcome or the receipt arrives.
  const outcome = done?.outcome === 'elsewhere' ? (fromReceipt ?? 'elsewhere') : (done?.outcome ?? fromReceipt)
  const running = !tool.interrupted && (tool.pending || !result)
  // A card waiting with no form in this window (attached late, or the session is another engine's): ask for its forms.
  const missing = running && !form && !done
  useEffect(() => {
    if (missing) attachSession(session)
  }, [missing, session])

  if (outcome) {
    return (
      <div className={cn(SHELL, 'px-4 py-3')}>
        <ElementFormOutcome outcome={outcome} origin={done?.origin ?? null} receipt={receipt} focus={answered} />
      </div>
    )
  }
  if (!form || readOnly) {
    return (
      <div className={cn(SHELL, 'flex min-w-0 items-center gap-2 px-4 py-3')}>
        <Lock className="size-4 shrink-0 text-fg-secondary" aria-hidden="true" />
        <span className={cn('min-w-0 flex-1 wrap-anywhere', result?.isError ? 'text-danger' : 'text-fg-secondary')}>
          {result ? receiptLine(result.output) : readOnly && form ? 'A form is waiting for an answer' : 'Waiting for the form…'}
        </span>
        {running && <ToolSpinner />}
      </div>
    )
  }
  return <ElementFormPanel form={form} onAnswered={() => setAnswered(true)} />
}
