// Dictation adornment for every text field (54 §5 pipeline; `lib/dictation.ts` owns the store and the daemon
// calls). `Input`/`Textarea` mount it themselves for controlled or plain text fields; `DictationButton` is the
// standalone mic for a bare `<input>` (`field`) or a rich editor (`onText`).

import { Mic, Square } from 'lucide-react'
import { type RefObject, useCallback, useEffect, useRef } from 'react'
import { Button } from '@/components/ui/button'
import { cn } from 'cn'
import * as dictation from '@/lib/dictation'
import { LOCAL, useEngines } from '@/link/engines'
import { useConnection } from '@/store/connection'

type Field = HTMLInputElement | HTMLTextAreaElement

/** `<input type>`s that take dictated words; every other type (password, number, date, url, file…) keeps a plain field. */
export const TEXT_TYPES: Record<string, true> = { text: true, search: true }

const LAYOUT = /^(w-|min-w-|max-w-|flex-|grow|shrink|basis-|self-|order-|m[trblxyse]?-|col-span-|row-span-|min-h-|max-h-)/

/**
 * Split a field's `className`: what sizes its box (width, flex, margin, grid span — with any `sm:`/`pointer-coarse:`
 * variant) goes on the wrapper that also holds the mic, the rest stays on the field.
 */
export function splitLayout(className?: string): [box: string, field: string] {
  const box: string[] = []
  const field: string[] = []
  for (const c of className?.split(/\s+/).filter(Boolean) ?? []) (LAYOUT.test(c.slice(c.lastIndexOf(':') + 1)) ? box : field).push(c)
  return [box.join(' '), field.join(' ')]
}

/**
 * Put `value` into a field the way typing would. React's value tracker (an instance-level `value` setter) fires
 * `onChange` only when the DOM value differs from what it last saw; the field shows the live splice while
 * dictating, so the tracker is first pointed at something else, then the prototype setter writes the real value
 * behind its back and the bubbled `input` event lands. A plain field just holds the value.
 */
export function setFieldValue(el: Field, value: string, caret: number): void {
  el.value = value === '' ? ' ' : ''
  const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype
  Object.getOwnPropertyDescriptor(proto, 'value')?.set?.call(el, value)
  el.dispatchEvent(new Event('input', { bubbles: true }))
  el.focus()
  el.setSelectionRange(caret, caret)
}

export type FieldDictation = dictation.Box & {
  /** No connection, or the engine has no speech-to-text model: render no mic. */
  hidden: boolean
}

/**
 * Dictation for the field behind `ref`. `value`: the controlled value (`shown` replaces it while dictating);
 * `undefined` for a plain field, whose DOM value is set directly instead. `onText` (rich editors, no field):
 * the finished utterance goes to the caller whole.
 */
export function useFieldDictation(ref: RefObject<Field | null>, value: string | undefined, onText?: (text: string) => void): FieldDictation {
  const api = useConnection((s) => s.api)
  const mic = useEngines((s) => s.active) !== LOCAL
  const finish = useCallback(
    (v: string, caret: number) => {
      if (onText) onText(v)
      else if (ref.current) setFieldValue(ref.current, v, caret)
    },
    [ref, onText],
  )
  const box = dictation.useBox(api, value ?? '', finish, ref, mic)
  const plain = value === undefined && !onText
  // A plain field re-renders nothing on its own: mirror the live splice and the read-only lock into the DOM.
  // ponytail: a controlled bare `<input>` whose owner re-renders mid-dictation snaps back to its prop until the finish lands.
  useEffect(() => {
    const el = ref.current
    if (!el || !plain || !box.dictating) return
    el.readOnly = true
    el.value = box.shown
    return () => {
      el.readOnly = false
    }
  }, [ref, plain, box.dictating, box.shown])
  const m = box.dict.model.state
  return { ...box, hidden: api === null || m === 'missing' || m === 'failed' }
}

/**
 * The mic itself: press to start, press again to stop (silence and the daemon's max length also stop it). A
 * disabled reason is the tooltip (`title`), never a modal. `onPointerDown` keeps focus in the field so
 * blur-committing inputs (renames) survive the press.
 */
export function MicButton({ d, className }: { d: FieldDictation; className?: string }) {
  if (d.hidden) return null
  const off = d.reason !== null && !d.dictating
  return (
    <Button
      type="button"
      variant="ghost"
      size="icon-xs"
      className={cn('rounded-full text-fg-secondary', off && 'opacity-50', d.dictating && 'bg-fg-accent/10 text-fg-accent', className)}
      aria-label={d.dictating ? 'Stop dictation' : 'Dictate'}
      aria-disabled={off}
      aria-pressed={d.dictating}
      title={d.reason ?? (d.dictating ? 'Stop dictation' : 'Dictate')}
      onPointerDown={(e) => e.preventDefault()}
      onClick={d.toggle}
    >
      {d.dictating ? <Square className="size-3 animate-pulse fill-current" /> : <Mic className="size-3.5" />}
    </Button>
  )
}

/**
 * Standalone mic. `field`: a bare `<input>`/`<textarea>` (controlled or not) that gets the text at its caret.
 * `onText`: a rich editor; the finished utterance arrives whole and the caller inserts it at its cursor.
 */
export function DictationButton({ field, onText, className }: { field?: RefObject<Field | null>; onText?: (text: string) => void; className?: string }) {
  const none = useRef<Field | null>(null)
  return <MicButton d={useFieldDictation(field ?? none, undefined, onText)} className={className} />
}
