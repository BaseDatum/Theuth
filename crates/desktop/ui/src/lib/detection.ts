// 158 §5 (W1): the detection prompt's pure half, shared by the Meetings-page banner and the start form —
// the one lawful sentence (R3, Amendment A2), what a one-tap Start sends (§5.4), and how the banner's state
// follows the bus (`Detected` / `DetectionCleared` / `AppReleased`, §4.5). Pure so the tests need no DOM.

import type { Detection } from '@/rpc/bindings/Detection'
import type { MeetingEvent } from '@/rpc/bindings/MeetingEvent'
import type { StartMeeting } from '@/rpc/bindings/StartMeeting'

/** R3, verbatim — a sentence, never a checkbox; Start sends `consent: true`. */
export const LAWFUL = 'By starting the recording you acknowledge the recording is lawful.'

/** §5.4: the detected app's pid, `consent: true`, `source: Detected`; every default is the engine's (§6). */
export function detectedStart(d: Detection): StartMeeting {
  return {
    process: d.app.pid,
    title: d.event?.event.title ?? null,
    context: null,
    consent: true,
    session_model: null,
    source: { kind: 'detected', app_id: d.app_id },
    ...(d.event ? { event: d.event.event.id } : {}),
    ...(d.project ? { project: d.project } : {}),
  }
}

/** §5.2 (d): the app let go while `meeting` records. */
export type Released = Extract<MeetingEvent, { type: 'AppReleased' }>

/** `live`: the bus has spoken about the prompt, so the initial `meeting_detection` answer no longer applies. */
export type BannerState = { detection: Detection | null; released: Released | null; live: boolean }

export const EMPTY: BannerState = { detection: null, released: null, live: false }

/** The banner's inputs: the bus and the "Keep recording" click. */
export type BannerAction = MeetingEvent | { type: 'Keep' }

/** §4.4/§4.5 as the banner sees it: a prompt per app id; a release prompt per recording meeting; `Started` ends the prompt. */
export function reduce(s: BannerState, e: BannerAction): BannerState {
  switch (e.type) {
    case 'Detected': {
      const { type: _, ...detection } = e
      return { ...s, detection, live: true }
    }
    case 'DetectionCleared':
      // A prompt the bus never showed (the banner is still on the RPC answer) clears too: `live` ends the seed.
      return s.detection && s.detection.app_id !== e.app_id ? s : { ...s, detection: null, live: true }
    case 'AppReleased':
      return { ...s, released: e }
    case 'Keep':
      return { ...s, released: null }
    case 'Started':
      return { ...s, detection: null, live: true }
    case 'State':
      return s.released?.meeting === e.meeting && e.state !== 'Recording' ? { ...s, released: null } : s
    default:
      return s
  }
}
