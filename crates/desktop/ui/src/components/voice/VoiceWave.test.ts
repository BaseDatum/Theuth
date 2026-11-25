import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { RecordingBox, VoiceWave } from '@/components/voice/VoiceWave'

const wave = (state: Parameters<typeof VoiceWave>[0]['state']) => renderToStaticMarkup(createElement(VoiceWave, { source: null, state, label: 'L' }))

describe('VoiceWave', () => {
  it('paints the waveform in the text colour, pulses only while thinking, and is labelled', () => {
    const thinking = wave('thinking')
    expect(thinking).toContain('data-slot="waveform"')
    expect(thinking).toContain('text-fg')
    expect(thinking).toContain('data-loading=""')
    expect(thinking).toContain('aria-label="L"')
    expect(thinking).toContain('<canvas')
    expect(wave('listening')).not.toContain('data-loading')
    expect(wave('idle')).not.toContain('data-loading')
  })
})

describe('RecordingBox', () => {
  const box = (recording: boolean) => renderToStaticMarkup(createElement(RecordingBox, { recording, source: null, children: createElement('textarea', { id: 'ta' }) }))

  it('hides the box and shows the wave while recording; the box alone otherwise', () => {
    const on = box(true)
    expect(on).toMatch(/<div class="invisible" aria-hidden="true"><textarea id="ta">/)
    expect(on).toContain('aria-label="Recording"')
    expect(on).toContain('data-slot="waveform"')
    expect(on).toContain('min-h-14')
    const off = box(false)
    expect(off).toMatch(/<div><textarea id="ta">/)
    expect(off).not.toContain('waveform')
  })
})
