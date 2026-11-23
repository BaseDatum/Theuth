// 212: a `show_file` file or a `show_diff` diff in its own window (`lib/popout.ts` kinds `file`, `diff`; routes
// `/file/$session`, `/diff/$session`), under the `Popped` root: the tile's header and body without the pane chrome.
import { type ReactNode, useEffect } from 'react'
import { DiffBody, DiffLayoutToggle, diffMeta, diffTitle, useDiffLayout, useDiffView } from '@/components/code/DiffBody'
import { FileView, fileMeta, highlightedLines, langOf, linesOf, useFileFollow, useFileView } from '@/components/code/FileView'
import { ViewControls } from '@/components/tiles/kinds/file'
import type { DiffSpec } from '@/rpc/bindings/method.artifact.input'

/** The window's header (the OS title bar sits above it, so no traffic-light inset) over its full-height body. */
function Frame({ title, meta, controls, children }: { title: string; meta?: string; controls: ReactNode; children: ReactNode }) {
  useEffect(() => {
    document.title = title
  }, [title])
  return (
    <div className="flex h-full min-h-0 min-w-0 flex-1 flex-col">
      <header className="flex h-[52px] shrink-0 items-center gap-1 border-b px-2">
        <div className="flex min-w-0 flex-1 flex-col justify-center px-1">
          <span className="truncate text-sm font-medium" title={title}>{title}</span>
          {meta && <span className="truncate text-xs text-fg-tertiary">{meta}</span>}
        </div>
        {controls}
      </header>
      <div className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">{children}</div>
    </div>
  )
}

/** A file alone, following the disk like its tile (212 §6): this window subscribes for itself. */
export function PoppedFile({ session, path, line, end }: { session: string; path: string; line?: number; end?: number }) {
  const q = useFileView(session, path)
  const { deleted } = useFileFollow(session, path)
  const data = q.data
  const meta = data
    ? fileMeta({ path, lang: langOf(data.lang, path), lines: data.lines, highlighted: highlightedLines(linesOf(data.text)), truncated: data.truncated, lossy: data.lossy, deleted })
    : path
  return (
    <Frame title={path.split(/[\\/]/).pop() || path} meta={meta} controls={<ViewControls refresh={() => void q.refetch()} fetching={q.isFetching} text={data?.text} what="file" />}>
      <FileView session={session} path={path} line={line} end={end} className="flex-1" />
    </Frame>
  )
}

/** A diff alone: the tree as it was when fetched, Refresh recomputes it; Unified | Split like its tile. */
export function PoppedDiff({ session, spec }: { session: string; spec: DiffSpec }) {
  const q = useDiffView(session, spec)
  const { layout, choose, ref } = useDiffLayout()
  const data = q.data
  return (
    <Frame
      title={diffTitle(spec)}
      meta={data ? diffMeta(data) : undefined}
      controls={
        <>
          <DiffLayoutToggle layout={layout} onChange={choose} className="mr-1" />
          <ViewControls refresh={() => void q.refetch()} fetching={q.isFetching} text={data?.text} what="patch" />
        </>
      }
    >
      <div ref={ref} className="flex min-h-0 flex-1 flex-col">
        <DiffBody session={session} spec={spec} layout={layout} className="flex-1" />
      </div>
    </Frame>
  )
}
