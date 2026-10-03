// 82 §A: theuth's one dictation / voice-note / talk waveform — the vendored audiocn electric waveform in the
// accent colour. `text-fg-accent` is the only colour hook: the line follows `currentColor` and the dark theme's
// white-hot core comes from `--electric-heat` under `.dark` (styles.css).

import { type CSSProperties, type ReactNode, useEffect, useRef, useState } from 'react'
import { ElectricWaveform } from '@/components/ui/electric-waveform'
import type { FrameSource, VisualFrame } from '@/lib/audio/types'
import { cn } from '@/lib/utils'
import type { VisualSource } from '@/lib/visual-source'

export type VoiceState = 'idle' | 'listening' | 'recording' | 'thinking' | 'speaking'

/** `source` null = the idle hum; `thinking` runs the loading pulse. Reduced motion is the component's job. */
export function VoiceWave({ source, state, className, style, label }: { source: FrameSource<VisualFrame> | null; state: VoiceState; className?: string; style?: CSSProperties; label: string }) {
  return (
    <ElectricWaveform
      mode="wave"
      source={source}
      loading={state === 'thinking'}
      arcs
      sparks
      intensity={state === 'recording' ? 1.1 : 0.9}
      className={cn('text-fg-accent h-full w-full', className)}
      style={style}
      aria-label={label}
    />
  )
}

/**
 * 82 §C: the composer's text region. While a voice note records the waveform IS the box: `children` (the textarea)
 * stay mounted but invisible, so the value, caret and height come back untouched when the recording ends.
 */
export function RecordingBox({ recording, source, children }: { recording: boolean; source: FrameSource<VisualFrame> | null; children: ReactNode }) {
  return (
    <div className={cn('relative', recording && 'min-h-14')}>
      <div className={recording ? 'invisible' : undefined} aria-hidden={recording || undefined}>
        {children}
      </div>
      {recording && <VoiceWave state="recording" source={source} label="Recording" className="absolute inset-0" />}
    </div>
  )
}

/**
 * Build a `VisualSource` while `enabled`, dispose it when not (or on unmount). `make` is read when the flag turns
 * on, so the tap it reaches for is the one that exists at that moment.
 */
export function useVoiceSource(enabled: boolean, make: () => VisualSource): FrameSource<VisualFrame> | null {
  const [source, setSource] = useState<FrameSource<VisualFrame> | null>(null)
  const makeRef = useRef(make)
  makeRef.current = make
  useEffect(() => {
    if (!enabled) return
    const s = makeRef.current()
    setSource(s.source)
    return () => {
      s.dispose()
      setSource(null)
    }
  }, [enabled])
  return source
}
