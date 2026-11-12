import { expect, test } from 'vitest'
import type { HarnessCommand } from "@/rpc/bindings/method.agent.output"
import type { SkillMeta } from "@/rpc/bindings/method.agent.output"
import { groupSkills, isEditable, repoDir, splitHarnessCommands } from './grouping'

const skill = (name: string, repo: string | null, description = '', path = `/data/skills/${repo ? `${repo}/` : ''}${name}.md`): SkillMeta => ({
  name,
  description,
  path,
  always: false,
  repo,
})

const SKILLS = [
  skill('zed', 'zoo-repo', 'editor rules'),
  skill('caveman', null, 'terse prose'),
  skill('alpha', 'abc-repo'),
  skill('ponytail', null, 'lazy code'),
  skill('beta', 'abc-repo', 'Caveman-adjacent'),
]

test('groups local first, then repos by name, skills sorted within a group', () => {
  expect(groupSkills(SKILLS, '').map((g) => [g.source, g.skills.map((s) => s.name)])).toEqual([
    ['Local', ['caveman', 'ponytail']],
    ['abc-repo', ['alpha', 'beta']],
    ['zoo-repo', ['zed']],
  ])
})

test('search matches name or description, case-insensitive, and drops empty groups', () => {
  expect(groupSkills(SKILLS, ' CAVE ').map((g) => [g.source, g.skills.map((s) => s.name)])).toEqual([
    ['Local', ['caveman']],
    ['abc-repo', ['beta']],
  ])
  expect(groupSkills(SKILLS, 'nothing')).toEqual([])
})

test('only a local <name>.md can be rewritten; SKILL.md directories and repo skills cannot', () => {
  expect(isEditable(skill('a', null))).toBe(true)
  expect(isEditable(skill('a', null, '', '/data/skills/a/SKILL.md'))).toBe(false)
  expect(isEditable(skill('a', 'r'))).toBe(false)
})

test('repoDir is the clone directory of a repo skill', () => {
  expect(repoDir(skill('a', 'r', '', '/data/skills/r/deep/a.md'))).toBe('/data/skills/r')
  expect(repoDir(skill('a', null))).toBe(null)
})

test('harness manifest (75 §3): omp `skill:` entries are skills by bare name, the rest commands; opencode bare names are all commands', () => {
  const cmd = (name: string, description: string, hint?: string): HarnessCommand => (hint === undefined ? { name, description } : { name, description, hint })
  const omp = [cmd('skill:ponytail', 'lazy code'), cmd('/ponytail', 'switch level', 'lite|full|ultra'), cmd('skill:caveman', 'terse prose'), cmd('/ponytail-review', 'review for bloat')]
  expect(splitHarnessCommands(omp)).toEqual({
    skills: [
      { name: 'ponytail', description: 'lazy code' },
      { name: 'caveman', description: 'terse prose' },
    ],
    commands: [omp[1], omp[3]],
  })
  expect(splitHarnessCommands(omp, ' REVIEW ')).toEqual({ skills: [], commands: [omp[3]] })
  expect(splitHarnessCommands(omp, 'prose')).toEqual({ skills: [{ name: 'caveman', description: 'terse prose' }], commands: [] })
  const opencode = [cmd('ponytail', 'lazy code'), cmd('init', 'create AGENTS.md')]
  expect(splitHarnessCommands(opencode)).toEqual({ skills: [], commands: opencode })
})
