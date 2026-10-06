import type { ResumeBanner as State } from '@/store/sessions'
import { useSessionField } from '@/store/sessions'

const hhmm = (ms: number): string => new Date(ms).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })

/** 109 §8.2: the one line the banner says for a resume state; the counts fill in once the `ResumeReport` lands. */
export function resumeText(r: State): string {
  const at = hhmm(r.at)
  if (r.kind === 'suspended') return r.resumed ? `Paused for an update at ${at} — resumed` : `Paused for an update at ${at} — resumes after the restart`
  const head = `Interrupted by a restart at ${at} — `
  if (!r.resumed && !r.report) return `${head}resuming…`
  if (!r.report) return `${head}resumed automatically`
  const { replayed, skipped, lostJobs } = r.report
  const parts = [`${replayed} tool ${replayed === 1 ? 'call' : 'calls'} replayed`]
  if (skipped.length) parts.push(`${skipped.length} skipped: ${skipped.join(', ')}, effect unknown`)
  if (lostJobs) parts.push(`${lostJobs} ${lostJobs === 1 ? 'job' : 'jobs'} lost`)
  return `${head}resumed automatically (${parts.join(', ')})`
}

/** Above the composer, one line, no modal (71/101): what the restart did to this session. Nothing when no turn was open. */
export function ResumeBanner({ session }: { session: string }) {
  const resume = useSessionField(session, 'resume')
  if (!resume) return null
  const tone = resume.report?.skipped.length ? 'text-warning' : 'text-fg-tertiary'
  return (
    <div className="mx-auto flex w-full shrink-0 items-center gap-2 px-4 py-1 text-xs md:max-w-[768px]">
      <span role="status" className={`min-w-0 flex-1 truncate ${tone}`} title={resumeText(resume)}>
        {resumeText(resume)}
      </span>
    </div>
  )
}
