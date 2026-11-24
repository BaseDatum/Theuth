import type { Api } from '@/rpc/bindings/client'
import { sendSkills, useSkillDraft } from '@/lib/drafts'
import { activeFor, activeSuffix, useSuffixes, withSuffix } from '@/lib/suffixes'

/**
 * A send with parked skills: ONE `use_skill(s)` input carrying every skill parked on `session`, then `words` closed by
 * the chat's prompt suffix (`suffixChat`'s, exactly as a plain send's body gets it). Afterwards the skills are gone and
 * a `once` suffix is spent, so nothing waits for the next message; an `always` suffix stays on. The skills leave the
 * draft before the call (a failure does not resend them), the suffix only once the engine took the input.
 */
export async function sendParked(api: Pick<Api, 'use_skill' | 'use_skills'>, session: string, words: string, suffixChat: string): Promise<string> {
  const skills = useSkillDraft.getState().by[session] ?? []
  const s = useSuffixes.getState()
  const suffix = activeSuffix(s.list, activeFor(s.active[suffixChat], s.always[suffixChat]))
  useSkillDraft.getState().clear(session)
  const id = await sendSkills(api, session, skills, withSuffix(words, suffix?.text))
  useSuffixes.getState().sent(suffixChat)
  return id
}
