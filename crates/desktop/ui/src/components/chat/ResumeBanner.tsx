import { useMesh } from '@/components/tasks/queries'
import { Button } from '@/components/ui/button'
import { useAttentionActions } from '@/components/hud/queries'
import { engineLabel } from '@/lib/owner'
import type { ResumeBanner as State } from '@/store/sessions'
import { useSessionField } from '@/store/sessions'

const hhmm = (ms: number): string => new Date(ms).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })

/** The counts once the `ResumeReport` lands: ` (2 tool calls replayed, 1 skipped: …, 1 job lost)`; empty before. */
function counts(r: State): string {
  if (!r.report) return ''
  const { replayed, skipped, lostJobs } = r.report
  const parts = [`${replayed} tool ${replayed === 1 ? 'call' : 'calls'} replayed`]
  if (skipped.length) parts.push(`${skipped.length} skipped: ${skipped.join(', ')}, effect unknown`)
  if (lostJobs) parts.push(`${lostJobs} ${lostJobs === 1 ? 'job' : 'jobs'} lost`)
  return ` (${parts.join(', ')})`
}

/**
 * 109 §8.2 / 139 §9.3: the one line the banner says for a resume state, first match wins. `label` renders an engine
 * id (the mesh name when known). A replica (`!here`) only says who runs the turn; the owner says what its resume did.
 */
export function resumeText(r: State, label: (engine: string) => string = (e) => e): string {
  const at = hhmm(r.at)
  if (!r.here) {
    const owner = r.owner ? label(r.owner) : 'another engine'
    if (r.kind === 'interrupted' || r.resumed) return `Running on ${owner}`
    if (r.cause === 'transfer') return `Moving to ${owner}…`
    return `Paused on ${owner} — resumes there`
  }
  if (r.cause === 'transfer' || r.cause === 'takeover') {
    const from = r.from ? label(r.from) : 'another engine'
    const head = `Taken from ${from} at ${at} — `
    if (r.cause === 'takeover') {
      return r.resumed
        ? `${head}resumed here; ${from} may still have been working, its unshipped work was discarded${counts(r)}`
        : `${head}resumes when you continue here`
    }
    // The owner side of a release (`TurnSuspended{release}`) and a taker under `resume: ask` share this state.
    if (!r.resumed) return `Paused for a transfer at ${at} — resumes on the new owner`
    return `${head}resumed here${counts(r)}`
  }
  if (r.kind === 'suspended') return r.resumed ? `Paused for an update at ${at} — resumed` : `Paused for an update at ${at} — resumes after the restart`
  const head = `Interrupted by a restart at ${at} — `
  if (!r.resumed && !r.report) return `${head}resuming…`
  return `${head}resumed automatically${counts(r)}`
}

/** 139 §9.3: the takeover left the turn open here (D5); the Resume beside the line is the `resume_turn` call. */
export const offersResume = (r: State): boolean => r.here && r.cause === 'takeover' && !r.resumed

/** Above the composer, one line, no modal (71/101): what the restart did to this session. Nothing when no turn was open. */
export function ResumeBanner({ session }: { session: string }) {
  const resume = useSessionField(session, 'resume')
  const { data: mesh } = useMesh()
  const { resume: resumeTurn } = useAttentionActions()
  if (!resume) return null
  const text = resumeText(resume, (e) => engineLabel(e, mesh?.peers))
  const tone = resume.report?.skipped.length ? 'text-warning' : 'text-fg-tertiary'
  return (
    <div className="mx-auto flex w-full shrink-0 items-center gap-2 px-4 py-1 text-xs md:max-w-[768px]">
      <span role="status" className={`min-w-0 flex-1 truncate ${tone}`} title={text}>
        {text}
      </span>
      {offersResume(resume) && (
        <Button size="sm" variant="outline" disabled={resumeTurn.isPending} onClick={() => resumeTurn.mutate(session)}>
          Resume
        </Button>
      )}
    </div>
  )
}
