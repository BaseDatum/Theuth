import type { Api } from '@/rpc/bindings/client'
import type { ApprovalsState } from '@/rpc/bindings/method.agent.output'

/**
 * A session's approval level, as the engine has it (183 §12.2, W-4): its own pin `on` or `off`, or none (`default`:
 * inherit an ancestor's pin, else the owner machine's `tools.approvals`). There is no finer per-session level: the
 * per-risk split (`tools.policy`: read_only / mutating / destructive) is machine-wide config.
 */
export type ApprovalLevel = 'on' | 'off' | 'default'

export type ApprovalsReading = Pick<ApprovalsState, 'global'> & { session?: boolean; pin?: ApprovalsState['pin'] | null }

export type ApprovalsChoice = {
  value: ApprovalLevel | undefined
  /** `label`: the ⋯ menu's short word; `long`: the composer shield's popover line. */
  options: { value: ApprovalLevel; label: string; long: string }[]
  disabled: boolean
  note?: string
}

/**
 * The levels to offer and the current one: the ⋯ menu's Approvals row and the composer shield's popover read the same.
 * `owner`: the owner's label for a session owned elsewhere; `offline` greys the setter; `oldOwner`: a pre-183 owner.
 */
export function approvalsChoice(o: { owner?: string; offline?: boolean; approvals?: ApprovalsReading; oldOwner?: boolean }): ApprovalsChoice {
  const off = !!o.offline && o.owner !== undefined
  // §12.2 (R1): what inherit gives is the EFFECTIVE value (`session`: an ancestor's pin can differ from the machine's switch)
  // when this session has no pin of its own; with a pin set, the machine's switch is the best the reader tells us.
  const inherited = o.approvals ? (o.approvals.pin == null ? (o.approvals.session ?? o.approvals.global) : o.approvals.global) : undefined
  const machine = inherited === undefined ? '' : ` (${inherited ? 'on' : 'off'})`
  const def = `${o.owner === undefined ? 'Machine' : o.owner} default${machine}`
  return {
    // W-4 tri-state: the row's own pin (`ApprovalsState.pin`; the engine omits it, older builds sent null) — none inherits.
    value: o.approvals && (o.approvals.pin == null ? 'default' : o.approvals.pin ? 'on' : 'off'),
    options: [
      { value: 'on', label: 'On', long: 'Ask before risky tools' },
      { value: 'off', label: 'Off', long: 'Never ask (off)' },
      { value: 'default', label: def, long: def },
    ],
    disabled: off || !o.approvals || !!o.oldOwner,
    note: o.oldOwner && o.owner !== undefined ? `${o.owner} needs an update` : undefined,
  }
}

/** The one RPC that sets a session's level: a pin (`on`/`off`), or `clear` back to the default (`enabled` is then ignored). */
export function setApprovalsLevel(api: Api, session: string, level: ApprovalLevel, global: boolean | undefined) {
  return level === 'default' ? api.set_approvals(session, global ?? true, true) : api.set_approvals(session, level === 'on')
}
