import type { SkillMeta } from '@/rpc/bindings/SkillMeta'

export const LOCAL = 'Local'

/**
 * The `/skills` list: skills whose name or description contains `query` (case-insensitive), grouped by
 * source — local files first, then each cloned repo by name — with the skills of a group sorted by name.
 */
export function groupSkills(skills: readonly SkillMeta[], query: string): { source: string; skills: SkillMeta[] }[] {
  const q = query.trim().toLowerCase()
  const groups = new Map<string, SkillMeta[]>()
  for (const s of skills) {
    if (q && !s.name.toLowerCase().includes(q) && !s.description.toLowerCase().includes(q)) continue
    const source = s.repo ?? LOCAL
    groups.set(source, [...(groups.get(source) ?? []), s])
  }
  return [...groups]
    .sort(([a], [b]) => (a === LOCAL ? -1 : b === LOCAL ? 1 : a.localeCompare(b)))
    .map(([source, list]) => ({ source, skills: list.sort((a, b) => a.name.localeCompare(b.name)) }))
}

/** A local `<name>.md` skill: the only kind `add_skill` may rewrite (repo skills and `SKILL.md` directories are not). */
export function isEditable(s: SkillMeta): boolean {
  return s.repo === null && !/[\\/]SKILL\.md$/i.test(s.path)
}

/** The clone directory of a repo skill: `…/skills/<repo>/…` → `…/skills/<repo>`; `null` for local skills. */
export function repoDir(s: SkillMeta): string | null {
  if (s.repo === null) return null
  const at = s.path.lastIndexOf(`/${s.repo}/`)
  return at < 0 ? null : s.path.slice(0, at + s.repo.length + 1)
}
