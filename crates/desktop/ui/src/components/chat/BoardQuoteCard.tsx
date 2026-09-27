// Design 71 §2.9: the transcript's card for a select-to-ask message — the place chips (board · block · path:a-b |
// node | step | stale), the quote as a mono block, then the question as the user's words. Detected by `MessageUser`
// after the `[attached …]` markers are split off (`lib/board-ask.parseAsk`).
import { Link } from '@tanstack/react-router'
import { MessageSquareQuote } from 'lucide-react'
import { type BoardAsk, formatAt } from '@/lib/board-ask'

export function BoardQuoteCard({ ask }: { ask: BoardAsk }) {
  const { ref, quote, question } = ask
  return (
    <div className="my-1 w-full max-w-[85%] min-w-0 rounded-xl border border-border-subtle bg-layer-1 px-4 py-3 text-sm md:max-w-[70%]" data-board-quote={ref.doc}>
      <div className="flex flex-wrap items-center gap-2 text-xs text-fg-secondary">
        <MessageSquareQuote className="size-3.5 shrink-0" aria-hidden="true" />
        <Link to="/notes/$id" params={{ id: ref.doc }} className="rounded-full bg-layer-2 px-2 font-mono leading-5 hover:text-foreground" title={ref.doc}>
          board {ref.doc.slice(0, 8)}
        </Link>
        {ref.block && (
          <span className="rounded-full bg-layer-2 px-2 font-mono leading-5" title={ref.block}>
            block {ref.block.slice(0, 8)}
          </span>
        )}
        {ref.at && <span className="rounded-full bg-layer-2 px-2 font-mono leading-5 wrap-anywhere">{formatAt(ref.at)}</span>}
      </div>
      {quote && <pre className="mt-2 overflow-x-auto rounded-md border-l-2 border-fg-accent bg-layer-2 px-3 py-2 font-mono text-xs leading-5 text-fg-secondary">{quote}</pre>}
      {question && <div className="mt-2 leading-6 wrap-anywhere whitespace-pre-wrap">{question}</div>}
    </div>
  )
}
