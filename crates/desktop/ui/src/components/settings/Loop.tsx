import { ConfigFlagRow } from '@/components/settings/ConfigFlagRow'
import { Count, Row, Section, refreshInfo, useAction } from '@/components/settings/shared'
import { useApi } from '@/lib/rpc'
import { useConnection } from '@/store/connection'

/** Settings › Agent › Loop (188 A8): how a turn runs — model calls, recovery, routine run time; 212 §14: memory peeks, hints, status, titles and next-step suggestions. */
export function Loop() {
  const api = useApi()
  const info = useConnection((s) => s.info)
  const setSubagentTimeout = useAction((seconds: number) =>
    api.set_subagent_timeout(seconds).then(() => refreshInfo(api)),
  )
  const setScheduleRunTimeout = useAction((seconds: number) =>
    api.set_schedule_run_timeout(seconds).then(() => refreshInfo(api)),
  )
  const setMaxToolRounds = useAction((rounds: number) =>
    api.set_max_tool_rounds(rounds).then(() => refreshInfo(api)),
  )
  const setStreamIdle = useAction((seconds: number) =>
    api.set_stream_idle(seconds).then(() => refreshInfo(api)),
  )
  const setMaxSalvages = useAction((count: number) =>
    api.set_max_salvages(count).then(() => refreshInfo(api)),
  )
  const setPtyIdle = useAction((seconds: number) =>
    api.set_pty_idle(seconds).then(() => refreshInfo(api)),
  )
  const setPeekThreshold = useAction((value: number) =>
    api.set_peek_threshold(value).then(() => refreshInfo(api)),
  )
  const setHintLimit = useAction((count: number) =>
    api.set_hint_limit(count).then(() => refreshInfo(api)),
  )
  const setStatusEvery = useAction((count: number) =>
    api.set_status_every(count).then(() => refreshInfo(api)),
  )

  return (
    <>
      <Section title="Model calls">
        <Row
          label="Stream idle timeout"
          description="Seconds without any output from the model before the call is abandoned as a dead connection. Raise it if long tool arguments stall on your provider. New sessions only."
        >
          <Count value={info?.stream_idle_secs} min={1} action={setStreamIdle} />
        </Row>
        <Row
          label="Reply recoveries per turn"
          description="How many times per turn theuth picks up a reply cut off by a stalled stream or the output token limit before the turn stops with an error. 0 turns recovery off. New sessions only."
        >
          <Count value={info?.max_salvages} min={0} action={setMaxSalvages} />
        </Row>
        <Row
          label="Tool rounds per turn"
          description="Tool-call rounds a turn may run before the model is asked to reply with text. New sessions only."
        >
          <Count value={info?.max_tool_rounds} min={1} action={setMaxToolRounds} />
        </Row>
        <Row
          label="Subagent timeout"
          description="Maximum runtime for a newly spawned subagent turn, in seconds."
        >
          <Count value={info?.subagent_timeout_secs} min={1} action={setSubagentTimeout} />
        </Row>
        <Row
          label="Terminal idle wake"
          description="Seconds a PTY job's screen must stay unchanged before the model is woken with <job_idle> and the screen (a REPL at its prompt, a wizard waiting, a stalled build). Applies to later `start` calls; 0 turns it off, and `start … idle_secs` overrides per job."
        >
          <Count value={info?.pty_idle_secs} min={0} action={setPtyIdle} />
        </Row>
      </Section>
      <Section title="Routines">
        <Row
          label="Run time limit"
          description="Seconds a routine run may take before it is cancelled and its history row says timeout. A routine's own run_timeout: overrides it. Applies to runs fired from now on."
        >
          <Count value={info?.schedule_run_timeout_secs || undefined} min={30} action={setScheduleRunTimeout} />
        </Row>
      </Section>
      <Section title="Memory and status">
        <ConfigFlagRow
          flag="knowledge.peek"
          name="Memory peeks"
          label="Memory peeks"
          description="Lets the agent check hinted memories against its question with the local decision model before reading them. Off removes peek_memory and its mention from the hints."
        />
        <Row
          label="Peek relevance threshold"
          description="How sure the local model must be that a memory answers the question. Higher pulls in fewer memories."
        >
          <Count
            aria-label="Peek relevance threshold"
            value={info?.peek_threshold || undefined}
            min={0.05}
            max={0.95}
            step={0.05}
            action={setPeekThreshold}
          />
        </Row>
        <Row
          label="Memory hints per nudge"
          description="How many related memories the harness suggests at a time. 0 turns memory hints off."
        >
          <Count aria-label="Memory hints per nudge" value={info?.hint_limit} min={0} action={setHintLimit} />
        </Row>
        <ConfigFlagRow
          flag="agent.status_tool"
          name="Status updates"
          label="Status updates"
          description="Lets the agent set the status line above the chat input and retitle its session. Off removes set_status and its reminders."
        />
        <Row
          label="Status reminder every N tool calls"
          description="How often the harness reminds the agent to update its status. 0 turns the reminder off; the tool stays available."
        >
          <Count aria-label="Status reminder every N tool calls" value={info?.status_every} min={0} action={setStatusEvery} />
        </Row>
        <ConfigFlagRow
          flag="agent.auto_title"
          name="Automatic titles"
          label="Automatic titles"
          description="Lets the title model name and rename sessions. Off keeps the first-message title until you or the agent rename it."
        />
        <ConfigFlagRow
          flag="agent.suggestions"
          name="Next-step suggestions"
          label="Next-step suggestions"
          description="After each chat turn, the title model suggests two or three next messages above the input (What's next?). Off makes no call and shows none. New sessions only."
        />
      </Section>
    </>
  )
}
