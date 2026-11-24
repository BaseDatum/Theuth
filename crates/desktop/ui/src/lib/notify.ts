// 217 §1: the drop-in for the toast library this replaced, same call shapes. Every notice lands in the status bar's
// slot (the phone line on a phone) and its history; nothing covers the UI.

import { type NoticeAction, type NoticeKind, useNotices } from '@/store/notices'

export type { NoticeAction, NoticeKind }

export type NoticeOpts = {
  /** Same id replaces in place (progress → done/error). */
  id?: string | number
  /** Second line; shown in history, truncated in the slot. */
  description?: string
  /** Ms; `Infinity` is sticky until dismissed or acted on. */
  duration?: number
  action?: NoticeAction
  /** Second button, as toasts had. */
  cancel?: NoticeAction
}

let seq = 0

const emit =
  (kind: NoticeKind) =>
  (text: string, o: NoticeOpts = {}): string | number =>
    useNotices.getState().push({ ...o, id: o.id ?? `n${++seq}`, kind, text })

const base = emit('info')

export const notify = Object.assign((text: string, o?: NoticeOpts) => base(text, o), {
  message: base,
  info: base,
  success: emit('done'),
  warning: emit('warn'),
  error: emit('error'),
  loading: emit('progress'),
  dismiss: (id?: string | number) => useNotices.getState().dismiss(id),
})
