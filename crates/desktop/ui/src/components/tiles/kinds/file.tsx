import { Copy, RotateCw } from 'lucide-react'
import { toast } from 'sonner'
import { FileView, fileMeta, highlightedLines, langOf, linesOf, useFileFollow, useFileView } from '@/components/code/FileView'
import type { FileProps } from '@/components/tiles/open'
import { TileFrame } from '@/components/tiles/TileFrame'
import type { TileKind, TileRender } from '@/components/tiles/types'
import { Button } from '@/components/ui/button'
import { openPopout } from '@/lib/popout'
import { cn } from '@/lib/utils'

/** The tile header's Refresh (refetch what the tile shows) and Copy (`text`, once loaded). */
export function ViewControls({ refresh, fetching, text, what }: { refresh(): void; fetching: boolean; text: string | undefined; what: string }) {
  return (
    <>
      <Button variant="ghost" size="icon" className="rounded-lg text-fg-secondary" aria-label="Refresh" disabled={fetching} onClick={refresh}>
        <RotateCw className={cn('size-4', fetching && 'animate-spin')} />
      </Button>
      <Button
        variant="ghost"
        size="icon"
        className="rounded-lg text-fg-secondary"
        aria-label={`Copy ${what}`}
        disabled={text === undefined}
        onClick={() => text !== undefined && navigator.clipboard.writeText(text).then(() => toast.message('Copied'), () => toast.error('Copy failed'))}
      >
        <Copy className="size-4" />
      </Button>
    </>
  )
}

/** A `show_file` file beside its chat (212 §4, §6): it follows the file on disk while open; Refresh re-reads it now. */
function File({ tile, first, close }: TileRender) {
  const p = tile.props as FileProps
  const q = useFileView(p.session, p.path)
  const { deleted } = useFileFollow(p.session, p.path)
  const data = q.data
  const meta = data
    ? fileMeta({ path: p.path, lang: langOf(data.lang || p.lang, p.path), lines: data.lines, highlighted: highlightedLines(linesOf(data.text)), truncated: data.truncated, lossy: data.lossy, deleted })
    : p.path
  return (
    <TileFrame
      first={first}
      onClose={close}
      title={p.title}
      tab={p.path.split('/').pop() || p.title}
      meta={meta}
      controls={<ViewControls refresh={() => void q.refetch()} fetching={q.isFetching} text={data?.text} what="file" />}
      popOut={() => void openPopout({ kind: 'file', session: p.session, path: p.path, line: p.line, end_line: p.end_line })}
    >
      <FileView session={p.session} path={p.path} lang={p.lang} line={p.line} end={p.end_line} className="flex-1" />
    </TileFrame>
  )
}

export const fileTile: TileKind = {
  title: (tile) => (tile.props as FileProps).title,
  render: (p) => <File {...p} />,
}
