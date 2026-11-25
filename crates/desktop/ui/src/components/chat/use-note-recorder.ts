// 45 §7.2 / 89 VOICE-HOLD: the voice-note recorder, shared by the chat composer and the whiteboard pill (218). Owns the
// MediaRecorder, the early-release rule (a stop that beats `getUserMedia` is applied the moment the recorder exists),
// the `MIN_NOTE_MS` slip rule, the audio-session unlock/release and the analyser wave the UI shows while recording.
// The finished note goes to the `onNote` of the render that started the recording; a discard or a slip produces nothing.

import { useRef, useState } from 'react'
import type { FrameSource, VisualFrame } from '@/lib/audio/types'
import { MIN_NOTE_MS } from '@/lib/hold'
import { notify as toast } from '@/lib/notify'
import * as speech from '@/lib/speech'
import { analyserSource } from '@/lib/visual-source'

/** Recorder containers in preference order (45 §7.2): WebKit gives AAC in MP4, Chromium Opus in WebM; the engine decodes all. */
export const AUDIO_MIMES = ['audio/mp4', 'audio/webm;codecs=opus', 'audio/ogg;codecs=opus', 'audio/webm']

export type NoteRecorder = {
  recording: boolean
  /** The recorder stream's analyser; the wave shown while `recording`. */
  source: FrameSource<VisualFrame> | null
  record(): Promise<void>
  /** Stop: `discard` drops the note. Before the recorder exists, remembered and applied once it does. */
  stop(discard: boolean): void
  /** Free the analyser and its context (an unmount mid-recording, whose `onstop` may never come). */
  dropWave(): void
}

export function useNoteRecorder(onNote: (blob: Blob) => void): NoteRecorder {
  const latest = useRef(onNote)
  latest.current = onNote
  const recorder = useRef<{ rec: MediaRecorder; discard: boolean } | null>(null)
  /** A release (send / cancel) that beat `getUserMedia`: applied the moment the recorder exists. */
  const pendingStop = useRef<boolean | null>(null)
  const [recording, setRecording] = useState(false)
  /** 82 §C: the recorder stream's analyser on its own context. */
  const noteWave = useRef<{ dispose: () => void } | null>(null)
  const [source, setSource] = useState<FrameSource<VisualFrame> | null>(null)
  const dropWave = () => {
    noteWave.current?.dispose()
    noteWave.current = null
    setSource(null)
  }

  const stop = (discard: boolean) => {
    const r = recorder.current
    if (!r) {
      if (pendingStop.current === null || discard) pendingStop.current = discard
      return
    }
    r.discard = discard
    r.rec.stop()
  }

  const record = async () => {
    // A user gesture: the one moment iOS lets the reply's `AudioContext` start (45 §7.3). Released on `onstop`
    // or a failed start, so an idle session does not keep iOS's Now Playing up.
    speech.unlock('record')
    // The handler of the render that started the note (as the composer's own closure always was).
    const deliver = latest.current
    pendingStop.current = null
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true })
      const rec = new MediaRecorder(stream, {
        mimeType: AUDIO_MIMES.find((m) => MediaRecorder.isTypeSupported(m)) ?? '',
        audioBitsPerSecond: 32_000,
      })
      const chunks: Blob[] = []
      rec.ondataavailable = (e) => chunks.push(e.data)
      let startedAt = 0
      rec.onstop = () => {
        dropWave()
        for (const track of stream.getTracks()) track.stop()
        void speech.release('record')
        const discard = recorder.current?.discard ?? true
        recorder.current = null
        setRecording(false)
        if (discard) return
        // 89 VOICE-HOLD: a release within `MIN_NOTE_MS` of the start is a slip, not a message — no upload.
        if (performance.now() - startedAt < MIN_NOTE_MS) toast('Hold to record, release to send', { duration: 2000 })
        else deliver(new Blob(chunks, { type: rec.mimeType }))
      }
      // 82 §C: a dedicated context for the wave (speech.ts owns the audio session); closed with the recording.
      const actx = new AudioContext()
      const wave = analyserSource(actx, stream)
      noteWave.current = {
        dispose: () => {
          wave.dispose()
          void actx.close()
        },
      }
      recorder.current = { rec, discard: false }
      startedAt = performance.now()
      rec.start()
      setSource(wave.source)
      setRecording(true)
      // The finger already lifted (a hold released before the mic opened): stop with what was said, or discard.
      const early = pendingStop.current
      if (early !== null) {
        pendingStop.current = null
        stop(early)
      }
    } catch (e) {
      pendingStop.current = null
      void speech.release('record')
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }

  return { recording, source, record, stop, dropWave }
}
