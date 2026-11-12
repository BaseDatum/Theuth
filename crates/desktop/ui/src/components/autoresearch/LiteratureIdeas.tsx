import { BookOpen } from 'lucide-react'
import type { CitationRow } from "@/rpc/bindings/method.autoresearch.output"
import type { Idea as StudyIdea } from "@/rpc/bindings/method.autoresearch.output"

/** `host` of a URL, `www.` dropped (mirrors `literature::host`). */
export function host(url: string): string {
  const rest = url.includes('://') ? url.slice(url.indexOf('://') + 3) : url
  const h = rest.split(/[/?#]/)[0] ?? rest
  return h.startsWith('www.') ? h.slice(4) : h
}

/** 159 W5: the literature ideas of a study, each with a paper chip — its link and quote when the
 * citations are known (`StudyDetail.citations`, a hand-off to Z), the count otherwise — and the
 * experiments that tried it. Renders nothing when no idea's source is `literature`, so the ideas
 * list of a 140 study is unchanged. Owner: W5. */
export function LiteratureIdeas({ ideas, citations = [] }: { ideas: readonly StudyIdea[]; citations?: readonly CitationRow[] }) {
  const papers = ideas.filter((i) => i.source === 'literature')
  if (papers.length === 0) return null
  const byIdea = new Map(citations.map((c) => [Number(c.idea_id), c]))
  return (
    <div className="mb-2 rounded-md border border-border-subtle bg-layer-2/40 p-2" data-papers={papers.length}>
      <div className="mb-1 flex items-center gap-1 text-[11px] font-medium text-fg-secondary">
        <BookOpen className="size-3.5" aria-hidden />
        From the literature · {papers.length} cited idea{papers.length === 1 ? '' : 's'}
      </div>
      <ul className="flex flex-col gap-1.5">
        {papers.map((i) => {
          const c = byIdea.get(i.id)
          return (
            <li key={i.id} className="text-[13px]" data-idea={i.id}>
              <span className={i.status === 'closed' ? 'text-fg-tertiary line-through' : ''}>{i.text.replace(/^\[paper\]\s*/, '')}</span>{' '}
              <span className="text-[11px] text-fg-tertiary">[{i.category}]</span>
              {i.tried_in.length > 0 && (
                <span className="ml-1 text-[11px] text-fg-tertiary" title={`Tried in experiment${i.tried_in.length === 1 ? '' : 's'} ${i.tried_in.join(', ')}`}>
                  tried in #{i.tried_in.join(', #')}
                </span>
              )}
              <div className="mt-0.5 flex min-w-0 items-start gap-1 text-[12px] text-fg-secondary">
                {c ? (
                  <>
                    <a href={c.url} target="_blank" rel="noreferrer" className="shrink-0 rounded bg-info/10 px-1 text-[11px] text-info hover:underline" title={c.url}>
                      paper · {c.title.trim() || host(c.url)}
                    </a>
                    <q className="min-w-0 italic" title={`${c.title} — ${host(c.url)}`}>
                      {c.quote}
                    </q>
                  </>
                ) : (
                  <span className="rounded bg-info/10 px-1 text-[11px] text-info">paper</span>
                )}
              </div>
            </li>
          )
        })}
      </ul>
    </div>
  )
}
