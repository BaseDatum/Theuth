import { JobFilters, JobList, LiveJob, useJobs } from '@/components/chat/JobsSheet'
import { type JobLogProps, popOutTile } from '@/components/tiles/open'
import { TileFrame } from '@/components/tiles/TileFrame'
import type { TileKind, TileRender } from '@/components/tiles/types'

/** The session's background jobs (`JobsSheet`) beside the chat, polled while mounted; the counts are the header's meta. */
function Jobs({ tile, first, close }: TileRender) {
  const jobs = useJobs(tile.session, true)
  return (
    <TileFrame first={first} onClose={close} title="Jobs" meta={jobs.summary} popOut={popOutTile(tile)}>
      <JobFilters jobs={jobs} className="px-4 py-3" />
      <JobList jobs={jobs} />
    </TileFrame>
  )
}

export const jobsTile: TileKind = {
  title: () => 'Jobs',
  render: (p) => <Jobs {...p} />,
}

/** A `show_pty` background job (211 §5): its live, read-only log beside the chat; the terminal tile is for PTY jobs. */
export const jobLogTile: TileKind = {
  title: (tile) => `Job · ${(tile.props as JobLogProps).job}`,
  render: ({ tile, first, close }) => {
    const { session, job, label } = tile.props as JobLogProps
    return (
      <TileFrame first={first} onClose={close} title={`Job · ${job}`} meta={label} popOut={popOutTile(tile)}>
        <LiveJob session={session} job={job} className="flex-1 px-3 pb-3" />
      </TileFrame>
    )
  },
}
