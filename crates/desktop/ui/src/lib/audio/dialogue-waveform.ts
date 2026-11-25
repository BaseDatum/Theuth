// The Dialogue single-line waveform: a 1:1 port of DialogueMockup's `WaveformEngine`
// (dialogue_mockup/ui/waveform_widget.py). Idle "ocean" motion, voice packets injected at the right edge that travel
// left, a pressure sink that absorbs them at the left edge, a 3-tap smoothing pass and a tanh soft limiter.
// The constants and the math are the reference's; do not tune them here.

import type { VisualFrame } from '@/lib/audio/types'

const TAU = Math.PI * 2

export const WAVEFORM_CONSTANTS = Object.freeze({
  idlePrimarySpeed: 1.375,
  idleSecondarySpeed: 1.78125,
  idleAmplitudeScale: 0.26,
  idlePrimaryFreq: 0.78,
  idleSecondaryFreq: 1.34,
  idleSecondaryAmp: 0.18,
  edgeFalloffExponent: 1.16,

  voiceWavelength: 0.2,
  voiceTravelSpeed: 0.3,
  voiceSpeedDrivePush: 0.055,
  voiceSpeedPulsePush: 0.028,

  voiceInjectionCadence: 0.145,
  voiceMinCadence: 0.06,
  voiceMaxCadence: 0.22,
  voicePacketSpawnPos: 1.1,
  voicePacketSpread: 0.07,
  voicePacketMinSpread: 0.04,
  voicePacketMaxSpread: 0.132,
  voicePacketSpreadGrowth: 0.01,
  voicePacketMaxSpreadGrowth: 0.032,

  voicePhaseBend: 0.34,
  voicePhaseSpreadMult: 1.75,
  voiceDensityMin: 0.88,
  voiceDensityMax: 1.22,
  voiceDensityLevelPush: 0.24,
  voiceDensityDrivePush: 0.1,

  leftPressureZone: 0.24,
  leftPressureAbsorb: 1.1,
  leftPressurePinRelax: 0.24,
  leftPressurePinFloor: 0.18,
  leftPressureSpine: 0.2,
})

const C = WAVEFORM_CONSTANTS

/** Per-frame audio features, each 0..1. */
export interface AudioFeatures {
  level: number
  low: number
  mid: number
  high: number
  transient: number
  brightness: number
}

/** One voice packet travelling right → left across the line (positions in 0..1 line units). */
export interface VoicePacket {
  pos: number
  speed: number
  density: number
  strength: number
  spread: number
  spreadGrowth: number
  decay: number
  accent: number
}

export const SILENT_FEATURES: Readonly<AudioFeatures> = Object.freeze({ level: 0, low: 0, mid: 0, high: 0, transient: 0, brightness: 0 })

const clamp01 = (v: number) => Math.max(0, Math.min(1, v))

const copyFeatures = (into: AudioFeatures, from: Readonly<AudioFeatures>) => {
  into.level = from.level
  into.low = from.low
  into.mid = from.mid
  into.high = from.high
  into.transient = from.transient
  into.brightness = from.brightness
}

export class WaveformEngine {
  /** Features as drawn: `target` smoothed with per-band attack/release. */
  readonly render: AudioFeatures = { ...SILENT_FEATURES }
  /** Features of the latest update (silent when not recording). */
  readonly target: AudioFeatures = { ...SILENT_FEATURES }
  speechDrive = 0
  speechPulse = 0
  edgeOnset = 0
  leftPressure = 0
  timeOffset = 0
  elapsedTime = 0
  voicePhase = 0
  packets: VoicePacket[] = []
  packetGap: number = C.voiceInjectionCadence
  lastPacketAt = -1
  isRecording = false
  private scratch = new Float32Array(0)

  /** Back to the freshly constructed state: no packets, no motion history, clock at zero. */
  reset(): void {
    copyFeatures(this.render, SILENT_FEATURES)
    copyFeatures(this.target, SILENT_FEATURES)
    this.speechDrive = 0
    this.speechPulse = 0
    this.edgeOnset = 0
    this.leftPressure = 0
    this.timeOffset = 0
    this.elapsedTime = 0
    this.voicePhase = 0
    this.packets.length = 0
    this.packetGap = C.voiceInjectionCadence
    this.lastPacketAt = -1
    this.isRecording = false
  }

  private smooth(cur: number, tgt: number, dt: number, rise: number, fall: number): number {
    const speed = tgt > cur ? rise : fall
    const alpha = 1 - Math.exp(-speed * dt)
    return cur + (tgt - cur) * alpha
  }

  update(features: Readonly<AudioFeatures>, isRecording: boolean, deltaSeconds: number): void {
    this.isRecording = isRecording
    copyFeatures(this.target, isRecording ? features : SILENT_FEATURES)
    const dt = Math.min(Math.max(0, deltaSeconds), 0.05)
    this.elapsedTime += dt
    this.timeOffset += dt

    const t = this.target
    const r = this.render
    if (dt > 0) {
      r.level = this.smooth(r.level, t.level, dt, 12.0, 4.2)
      r.low = this.smooth(r.low, t.low, dt, 8.2, 3.6)
      r.mid = this.smooth(r.mid, t.mid, dt, 9.2, 4.1)
      r.high = this.smooth(r.high, t.high, dt, 10.2, 4.4)
      r.transient = this.smooth(r.transient, t.transient, dt, 15.0, 6.0)
      r.brightness = this.smooth(r.brightness, t.brightness, dt, 6.8, 3.6)
    }

    const driveTarget = Math.min(1, t.level * 0.96 + t.mid * 0.26 + t.transient * 0.38)
    const pulseTarget = Math.min(1, t.transient * 1.05 + Math.max(0, t.level - r.level * 0.58) * 0.85 + Math.max(0, t.mid - r.mid * 0.7) * 0.35)
    const onsetTarget = Math.min(1, t.transient * 1.12 + Math.max(0, t.level - r.level) * 1.1 + Math.max(0, t.mid - r.mid) * 0.4)

    this.speechDrive = this.smooth(this.speechDrive, driveTarget, dt, 10.0, 3.4)
    this.speechPulse = this.smooth(this.speechPulse, pulseTarget, dt, 18.0, 7.2)
    this.edgeOnset = this.smooth(this.edgeOnset, onsetTarget, dt, 22.0, 8.2)
    this.voicePhase += (TAU * C.voiceTravelSpeed * dt) / Math.max(0.12, C.voiceWavelength)

    this.advancePackets(dt)
  }

  private oceanEnvelope(pos: number): number {
    const p = clamp01(pos)
    return 0.5 + 0.5 * Math.sin(p * TAU * 0.46 + this.timeOffset * (C.idlePrimarySpeed * 0.72) + 0.35)
  }

  private advancePackets(dt: number): void {
    if (!this.isRecording) {
      this.speechDrive = 0
      this.speechPulse = 0
      this.edgeOnset = 0
      this.leftPressure = 0
      this.packetGap = C.voiceInjectionCadence
      this.packets.length = 0
      this.lastPacketAt = -1
      return
    }

    const r = this.render
    const drive = Math.min(1, this.speechDrive * 0.94 + r.mid * 0.2 + r.level * 0.08)
    const pulse = Math.min(1, this.speechPulse * 0.92 + r.transient * 0.22)
    const onset = Math.min(1, this.edgeOnset * 0.88 + pulse * 0.24)
    const age = this.lastPacketAt < 0 ? Number.POSITIVE_INFINITY : this.elapsedTime - this.lastPacketAt
    const canEmit = this.lastPacketAt < 0 || age >= this.packetGap || (onset > 0.58 && age >= 0.048)

    if (drive > 0.05 && canEmit) {
      const oceanPush = 0.92 + 0.12 * this.oceanEnvelope(1.0)
      const speed = Math.min(0.44, C.voiceTravelSpeed + this.speechDrive * C.voiceSpeedDrivePush + this.speechPulse * C.voiceSpeedPulsePush)
      const density = Math.max(C.voiceDensityMin, Math.min(C.voiceDensityMax, 0.88 + r.level * C.voiceDensityLevelPush + this.speechDrive * C.voiceDensityDrivePush))
      const spread = Math.max(C.voicePacketMinSpread, Math.min(C.voicePacketMaxSpread, C.voicePacketSpread + r.low * 0.028 + Math.max(0, drive - pulse) * 0.024 - pulse * 0.03 - r.brightness * 0.014))
      const spreadGrowth = Math.max(C.voicePacketSpreadGrowth, Math.min(C.voicePacketMaxSpreadGrowth, C.voicePacketSpreadGrowth + pulse * 0.02 + r.brightness * 0.008))
      this.packets.push({
        pos: C.voicePacketSpawnPos,
        speed,
        density,
        strength: Math.min(0.98, (0.08 + drive * 0.42 + pulse * 0.28 + r.level * 0.14) * oceanPush),
        spread,
        spreadGrowth,
        decay: Math.max(0.16, 0.34 - r.level * 0.07 - r.low * 0.04 + Math.max(0, 0.24 - pulse) * 0.12),
        accent: 0.84 + pulse * 0.34,
      })
      this.lastPacketAt = this.elapsedTime
      let nextGap = C.voiceInjectionCadence + Math.max(0, 0.24 - r.level) * 0.18 + Math.max(0, 0.38 - onset) * 0.12 - onset * 0.065
      if (onset > 0.72) nextGap -= 0.05
      else if (onset < 0.16 && drive < 0.2) nextGap += 0.035
      this.packetGap = Math.max(C.voiceMinCadence, Math.min(C.voiceMaxCadence, nextGap))
    }

    // Advance, absorb at the left sink, and drop spent packets (in place, order kept).
    let pressureTarget = 0
    let kept = 0
    for (const p of this.packets) {
      p.pos -= p.speed * dt
      p.strength *= Math.exp(-dt * p.decay)
      const lz = clamp01((C.leftPressureZone - p.pos) / C.leftPressureZone)
      if (lz > 0) {
        pressureTarget += p.strength * lz * (0.8 + (p.speed / C.voiceTravelSpeed) * 0.2)
        p.strength *= Math.exp(-dt * C.leftPressureAbsorb * lz)
      }
      p.spread = Math.min(0.2, p.spread + dt * p.spreadGrowth)
      if (p.pos > -0.3 && p.strength > 0.02) this.packets[kept++] = p
    }
    this.packets.length = kept
    this.leftPressure = this.smooth(this.leftPressure, clamp01(pressureTarget * 0.74), dt, 10.0, 1.5)
  }

  private restAt(pos: number, maxD: number): number {
    if (maxD <= 0) return 0
    const falloff = Math.sin(pos * Math.PI) ** C.edgeFalloffExponent
    const primary = Math.sin(pos * TAU * C.idlePrimaryFreq + this.timeOffset * C.idlePrimarySpeed)
    const secondary = Math.sin(pos * TAU * C.idleSecondaryFreq + this.timeOffset * C.idleSecondarySpeed + 0.55) * C.idleSecondaryAmp
    return (primary * 0.88 + secondary) * (maxD * C.idleAmplitudeScale) * falloff
  }

  private voiceAt(pos: number, maxD: number, restD: number, restComp: number): number {
    if (maxD <= 0 || this.packets.length === 0) return 0
    let env = 0
    let accent = 0
    let phaseBend = 0
    let dens = 0
    for (const p of this.packets) {
      const d = pos - p.pos
      const g = Math.exp(-(d * d) / (2 * p.spread * p.spread))
      env += p.strength * g
      accent = Math.max(accent, p.accent * g)
      dens += p.density * p.strength * g
      const ps = p.spread * C.voicePhaseSpreadMult
      const pg = Math.exp(-(d * d) / (2 * ps * ps))
      phaseBend += p.strength * (d / Math.max(ps, 1e-6)) * pg * C.voicePhaseBend
    }
    if (env <= 0) return 0

    const restRatio = restD / Math.max(1, maxD)
    const oceanEnv = this.oceanEnvelope(pos)
    const oceanPhase = restRatio * 0.46 + (oceanEnv - 0.5) * 0.22
    const shaped = 1 - Math.exp(-env * 0.95)
    const localBend = Math.max(-0.48, Math.min(0.48, phaseBend * (0.72 + shaped * 0.22)))
    const vDens = Math.max(C.voiceDensityMin, Math.min(C.voiceDensityMax, dens / Math.max(env, 1e-6)))
    const localDens = 1 + (vDens - 1) * Math.min(1, shaped * 0.92)
    const spatialPhase = (pos * TAU) / Math.max(0.12, C.voiceWavelength)
    const carrier = Math.sin(spatialPhase * localDens + this.voicePhase + oceanPhase + localBend)

    const baseLeft = Math.sin(clamp01(pos) * (Math.PI / 2)) ** 1.02
    const leftYield = this.leftPressure * Math.exp(-Math.max(0, pos) * 8.5)
    const leftAnchor = baseLeft * (1 - leftYield * C.leftPressurePinRelax) + leftYield * C.leftPressurePinFloor
    const oceanBias = Math.max(0.78, Math.min(1.18, 0.98 + restRatio * 0.28))
    const oceanLift = 0.88 + 0.22 * oceanEnv
    const accentScale = 0.72 + 0.34 * Math.min(1, accent)

    let spine = (restD * (0.18 + 0.16 * oceanEnv) + maxD * (oceanEnv - 0.5) * 0.12) * Math.min(0.42, 0.14 + shaped * 0.3)
    spine += (restComp * 0.22 + maxD * (oceanEnv - 0.5) * 0.08) * this.leftPressure * C.leftPressureSpine * Math.exp(-Math.max(0, pos) * 7.2)

    const headroom = Math.max(0, maxD * 0.96 - Math.abs(restComp + spine))
    const amp = headroom * shaped * 0.92 * leftAnchor * oceanBias * oceanLift * accentScale
    return spine + amp * carrier
  }

  private softLimit(v: number, maxD: number): number {
    if (maxD <= 0) return 0
    return Math.tanh(v / (maxD * 0.82)) * maxD * 0.92
  }

  /**
   * Vertical displacement (px, + = down) of `count` evenly spaced points across the line, limited to ±`maxD`.
   * Writes into `out` when it has room for `count` values (no allocation per frame), else into a new array.
   */
  displacements(count: number, maxD: number, out?: Float32Array): Float32Array {
    const dst = out && out.length >= count ? out : new Float32Array(count)
    if (count <= 0) return dst
    if (count === 1) {
      dst[0] = this.softLimit(this.restAt(0, maxD), maxD)
      return dst
    }
    if (this.scratch.length < count) this.scratch = new Float32Array(count)
    const raw = this.scratch
    const presence = Math.min(1, this.speechDrive * 1.12 + this.speechPulse * 0.42 + this.render.mid * 0.16)
    const restScale = Math.max(0.72, 1 - presence * 0.22)
    for (let i = 0; i < count; i++) {
      const pos = i / (count - 1)
      const rest = this.restAt(pos, maxD)
      const restComp = rest * restScale
      const voice = this.voiceAt(pos, maxD, rest, restComp)
      raw[i] = this.softLimit(restComp + voice, maxD)
    }
    dst[0] = raw[0]!
    dst[count - 1] = raw[count - 1]!
    for (let i = 1; i < count - 1; i++) dst[i] = raw[i - 1]! * 0.18 + raw[i]! * 0.64 + raw[i + 1]! * 0.18
    return dst
  }
}

const bandMean = (bands: Float32Array, from: number, to: number): number => {
  const a = Math.max(0, Math.min(bands.length, from))
  const b = Math.max(a, Math.min(bands.length, to))
  if (b === a) return 0
  let s = 0
  for (let i = a; i < b; i++) s += bands[i]!
  return s / (b - a)
}

/**
 * `VisualFrame` → the engine's features. Low/mid/high are the means of the bottom 25 %, the next 37.5 % and the top
 * 37.5 % of the (log-spaced, low→high) bands with the reference's per-range gains; `level` blends the band mix with
 * the frame's peak. `transient` is the rise over `prevLevel` (a smoothed level the caller threads through as
 * `nextLevel`); `brightness` is the high share of the spectrum.
 */
export function visualFrameToAudioFeatures(frame: VisualFrame, prevLevel: number): { features: AudioFeatures; nextLevel: number } {
  const n = frame.bands.length
  const lowEnd = Math.max(1, Math.round(n * 0.25))
  const midEnd = Math.max(lowEnd, Math.round(n * 0.625))
  const low = Math.min(1, bandMean(frame.bands, 0, lowEnd) * 1.6)
  const mid = Math.min(1, bandMean(frame.bands, lowEnd, midEnd) * 1.9)
  const high = Math.min(1, bandMean(frame.bands, midEnd, n) * 2.2)
  const bandLevel = Math.min(1, low * 0.35 + mid * 0.5 + high * 0.25)
  // Peak over −60..0 dBFS; silence (−∞, NaN) is 0.
  const peak = Number.isFinite(frame.peakDb) ? clamp01((frame.peakDb + 60) / 60) : 0
  const level = clamp01(bandLevel * 0.6 + peak * 0.4)
  const transient = clamp01((level - prevLevel) * 5.0)
  const sum = low + mid + high + 1e-4
  return {
    features: { level, low, mid, high, transient, brightness: Math.min(1, (mid * 0.4 + high) / sum) },
    nextLevel: level * 0.7 + prevLevel * 0.3,
  }
}
