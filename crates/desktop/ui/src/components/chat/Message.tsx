import { memo } from 'react'
import type { ApprovalRequested } from '@/rpc/bindings/ApprovalRequested'
import type { Entry } from '@/store/sessions'
import { ApprovalCard } from './ApprovalCard'
import { MessageAssistant } from './MessageAssistant'
import { MessageCompacted } from './MessageCompacted'
import { MessageError, MessageInfo } from './MessageInfo'
import { MessagePeer } from './MessagePeer'
import { MessageReasoning } from './MessageReasoning'
import { MessageTool } from './MessageTool'
import { MessageUser } from './MessageUser'

function body(entry: Entry) {
  switch (entry.kind) {
    case 'user':
      return <MessageUser text={entry.text} skill={entry.skill} files={entry.files} />
    case 'assistant':
      return <MessageAssistant text={entry.text} open={entry.open} />
    case 'reasoning':
      return <MessageReasoning text={entry.text} done={entry.done} />
    case 'tool':
      return <MessageTool tool={entry} />
    case 'peer':
      return <MessagePeer from={entry.from} text={entry.text} />
    case 'info':
      return <MessageInfo text={entry.text} />
    case 'error':
      return <MessageError text={entry.text} />
    case 'compacted':
      return <MessageCompacted epoch={entry.epoch} style={entry.style} summary={entry.summary} />
  }
}

/** One transcript row: the entry's card plus the approval cards waiting on it. Entries are immutable, so `memo` skips untouched rows. */
export const Message = memo(function Message({ entry, approvals }: { entry: Entry; approvals: ApprovalRequested[] | null }) {
  return (
    <>
      {body(entry)}
      {approvals?.map((a) => <ApprovalCard key={a.id} approval={a} />)}
    </>
  )
})
