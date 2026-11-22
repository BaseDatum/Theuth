import { JobFilters, JobList, useJobs } from '@/components/chat/JobsSheet'
import { TileFrame } from '@/components/tiles/TileFrame'
import type { TileKind, TileRender } from '@/components/tiles/types'

/** The session's background jobs (`JobsSheet`) beside the chat, polled while mounted; the counts are the header's meta. */
function Jobs({ tile, first, close }: TileRender) {
  const jobs = useJobs(tile.session, true)
  return (
    <TileFrame first={first} onClose={close} title="Jobs" meta={jobs.summary}>
      <JobFilters jobs={jobs} className="px-4 py-3" />
      <JobList jobs={jobs} />
    </TileFrame>
  )
}

export const jobsTile: TileKind = {
  title: () => 'Jobs',
  render: (p) => <Jobs {...p} />,
}
