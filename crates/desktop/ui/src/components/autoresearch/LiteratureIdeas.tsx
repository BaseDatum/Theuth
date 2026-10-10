import type { StudyIdea } from '@/rpc/bindings/StudyIdea'

/** 159 W5: a paper chip on each literature idea with its link and quote. Owner: W5.
 * Renders nothing when no idea's source is `literature`, so the ideas list is unchanged today. */
export function LiteratureIdeas({ ideas }: { ideas: readonly StudyIdea[] }) {
  const papers = ideas.filter((i) => i.source === 'literature')
  if (papers.length === 0) return null
  return (
    <div className="text-xs text-fg-tertiary" data-papers={papers.length}>
      {papers.length} literature idea{papers.length === 1 ? '' : 's'} (W5 is not built yet)
    </div>
  )
}
