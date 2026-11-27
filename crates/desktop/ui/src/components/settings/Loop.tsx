import { AutoArchive } from '@/components/settings/AutoArchive'
import { ConfigFlagRow } from '@/components/settings/ConfigFlagRow'
import { setterField, fromInfo } from '@/components/settings/copy/fields'
import { Count, Row, Section, refreshInfo, useAction } from '@/components/settings/shared'
import { useApi } from '@/lib/rpc'
import { Capability } from '@/rpc/capabilities'
import { useConnection } from '@/store/connection'

/** Settings › Agent › Loop (188 A8): how a turn runs — model calls, recovery, routine run time; 212 §14: memory peeks, hints, status, titles and next-step suggestions; auto-archive of inactive chats. */
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
          copy={setterField('stream-idle', 'Stream idle timeout', info?.stream_idle_secs, 'set_stream_idle', { read: fromInfo((i) => i.stream_idle_secs) })}
        >
          <Count value={info?.stream_idle_secs} min={1} action={setStreamIdle} />
        </Row>
        <Row
          label="Reply recoveries per turn"
          description="How many times per turn theuth picks up a reply cut off by a stalled stream or the output token limit before the turn stops with an error. 0 turns recovery off. New sessions only."
          copy={setterField('max-salvages', 'Reply recoveries per turn', info?.max_salvages, 'set_max_salvages', { read: fromInfo((i) => i.max_salvages) })}
        >
          <Count value={info?.max_salvages} min={0} action={setMaxSalvages} />
        </Row>
        <Row
          label="Tool rounds per turn"
          description="Tool-call rounds a turn may run before the model is asked to reply with text. New sessions only."
          copy={setterField('max-tool-rounds', 'Tool rounds per turn', info?.max_tool_rounds, 'set_max_tool_rounds', { read: fromInfo((i) => i.max_tool_rounds) })}
        >
          <Count value={info?.max_tool_rounds} min={1} action={setMaxToolRounds} />
        </Row>
        <Row
          label="Subagent timeout"
          description="Maximum runtime for a newly spawned subagent turn, in seconds."
          copy={setterField('subagent-timeout', 'Subagent timeout', info?.subagent_timeout_secs, 'set_subagent_timeout', { read: fromInfo((i) => i.subagent_timeout_secs) })}
        >
          <Count value={info?.subagent_timeout_secs} min={1} action={setSubagentTimeout} />
        </Row>
        <Row
          label="Terminal idle wake"
          description="Seconds a PTY job's screen must stay unchanged before the model is woken with <job_idle> and the screen (a REPL at its prompt, a wizard waiting, a stalled build). Applies to later `start` calls; 0 turns it off, and `start … idle_secs` overrides per job."
          copy={setterField('pty-idle', 'Terminal idle wake', info?.pty_idle_secs, 'set_pty_idle', { read: fromInfo((i) => i.pty_idle_secs) })}
        >
          <Count value={info?.pty_idle_secs} min={0} action={setPtyIdle} />
        </Row>
      </Section>
      <Section title="Routines">
        <Row
          label="Run time limit"
          description="Seconds a routine run may take before it is cancelled and its history row says timeout. A routine's own run_timeout: overrides it. Applies to runs fired from now on."
          copy={setterField('run-timeout', 'Run time limit', info?.schedule_run_timeout_secs || undefined, 'set_schedule_run_timeout', { read: fromInfo((i) => i.schedule_run_timeout_secs) })}
        >
          <Count value={info?.schedule_run_timeout_secs || undefined} min={30} action={setScheduleRunTimeout} />
        </Row>
      </Section>
      <Capability required={{ methods: ['set_auto_archive'] }} hide>
        <Section title="Chats">
          <AutoArchive />
        </Section>
      </Capability>
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
          copy={setterField('peek-threshold', 'Peek relevance threshold', info?.peek_threshold || undefined, 'set_peek_threshold', { read: fromInfo((i) => i.peek_threshold) })}
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
          copy={setterField('hint-limit', 'Memory hints per nudge', info?.hint_limit, 'set_hint_limit', { read: fromInfo((i) => i.hint_limit) })}
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
          copy={setterField('status-every', 'Status reminder every N tool calls', info?.status_every, 'set_status_every', { read: fromInfo((i) => i.status_every) })}
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
