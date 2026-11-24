// 153 §6.4: the dictionary suggestion card (mock frame D) under the Composer. The engine publishes
// `DictionarySuggestion` after a `dictation_correction` found the same single-word edit twice; Add / Not now / Never
// answer through `dictionary_suggestion_answer`, 20 s without an answer dismisses the card (the engine counts it as
// ignored at the next correction — no RPC). Play plays the dictation's audio (`dictation_audio_get`, base64 i16 16 kHz).

import { Play } from 'lucide-react'
import { useEffect, useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { Button } from '@/components/ui/button'
import { decode } from '@/lib/base64'
import { type Suggestion, clearSuggestion } from '@/lib/dictation'
import type { Api } from '@/rpc/bindings/client'
import type { SuggestionAnswer } from '@/rpc/bindings/method.talk.input'

/** §3.1 "suggesting": auto-dismiss after 20 s. */
export const DISMISS_MS = 20_000
const RATE = 16_000

/** Base64 little-endian i16 PCM → samples in [-1, 1). */
export function pcmSamples(b64: string): Float32Array<ArrayBuffer> {
  const bytes = decode(b64)
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength - (bytes.byteLength % 2))
  const out = new Float32Array(new ArrayBuffer(view.byteLength * 2))
  for (let i = 0; i < out.length; i++) out[i] = view.getInt16(i * 2, true) / 32768
  return out
}

/** Play 16 kHz samples once through a throwaway context (the browser resamples to the device rate). */
export async function playPcm(samples: Float32Array<ArrayBuffer>): Promise<void> {
  if (samples.length === 0) return
  const ctx = new AudioContext()
  const buffer = ctx.createBuffer(1, samples.length, RATE)
  buffer.copyToChannel(samples, 0)
  const src = ctx.createBufferSource()
  src.buffer = buffer
  src.connect(ctx.destination)
  src.onended = () => void ctx.close()
  src.start()
}

const times = (n: number) => (n === 1 ? 'once' : n === 2 ? 'twice' : `${n} times`)

export function DictionarySuggestion({ s, api, className }: { s: Suggestion; api: Api; className?: string }) {
  const [busy, setBusy] = useState(false)
  useEffect(() => {
    const t = window.setTimeout(() => clearSuggestion(s.id), DISMISS_MS)
    return () => window.clearTimeout(t)
  }, [s.id])
  const answer = async (a: SuggestionAnswer) => {
    setBusy(true)
    try {
      await api.dictionary_suggestion_answer(s.id, a)
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    } finally {
      clearSuggestion(s.id)
    }
  }
  const play = async () => {
    if (!s.dictationId) return
    try {
      await playPcm(pcmSamples(await api.dictation_audio_get(s.dictationId, null)))
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }
  return (
    <div role="status" aria-label="Dictionary suggestion" className={`rounded-xl border border-border-subtle bg-muted/40 px-3.5 py-3 text-sm ${className ?? ''}`}>
      <div className="flex items-center gap-2 text-xs text-fg-tertiary">
        <span>Dictionary</span>
        <span className="flex-1" />
        <span>{times(s.occurrences)} this week</span>
      </div>
      <p className="mt-2">
        You changed <s className="text-fg-tertiary">{s.heard}</s> → <code className="rounded bg-muted px-1 font-mono text-[12px]">{s.typed}</code> after dictating.
        <br />
        <span className="text-xs text-fg-tertiary">Heard the same way {times(s.occurrences)}. Add it as a replacement?</span>
      </p>
      <div className="mt-2 flex flex-wrap items-center gap-1.5">
        <Button size="sm" disabled={busy} onClick={() => void answer('add')}>
          Add
        </Button>
        <Button size="sm" variant="outline" disabled={busy} onClick={() => void answer('not_now')}>
          Not now
        </Button>
        <Button size="sm" variant="outline" disabled={busy} onClick={() => void answer('never')}>
          Never for this
        </Button>
        {s.hasAudio && s.dictationId && (
          <Button size="sm" variant="ghost" className="ml-auto text-fg-tertiary" onClick={() => void play()}>
            <Play className="size-3.5" /> play what you said
          </Button>
        )}
      </div>
    </div>
  )
}
