import { describe, expect, test } from 'vitest'
import type { Suffix } from '@/lib/suffixes'
import type { SkillMeta } from '@/rpc/bindings/method.agent.output'
import {
  filterPromptTriggers,
  filterSkillTriggers,
  inlineTrigger,
  insertExpansion,
  promptTrigger,
  skillTrigger,
  stripTrigger,
} from './composer-triggers'
import { pathTrigger } from './path-complete'

const skill = (name: string, description: string): SkillMeta => ({ name, description }) as SkillMeta
const prompt = (id: string, name: string, text: string): Suffix => ({ id, name, text })

describe('$ skill trigger', () => {
  test('fires at start of input and after whitespace, not mid-word', () => {
    expect(skillTrigger('$', 1)).toEqual({ start: 0, word: '' })
    expect(skillTrigger('$code_review', 12)).toEqual({ start: 0, word: 'code_review' })
    expect(skillTrigger('hello $code_review', 18)).toEqual({ start: 6, word: 'code_review' })
    expect(skillTrigger('line 1\n$foo.bar-1', 17)).toEqual({ start: 7, word: 'foo.bar-1' })
    expect(skillTrigger('a$b', 3)).toBeNull()
    expect(skillTrigger('price$10', 8)).toBeNull()
    expect(skillTrigger('$foo', 0)).toBeNull()
  })

  test('stops on slash, whitespace, another $, and shell punctuation so PATH-AC takes over', () => {
    expect(skillTrigger('$HOME', 5)).toEqual({ start: 0, word: 'HOME' })
    expect(pathTrigger('$HOME', 5)).toBeNull()
    expect(skillTrigger('$HOME/', 6)).toBeNull()
    expect(pathTrigger('$HOME/', 6)).toEqual({ start: 0, end: 6 })
    expect(skillTrigger('$HOME/.th', 9)).toBeNull()
    expect(pathTrigger('$HOME/.th', 9)).toEqual({ start: 0, end: 9 })
    expect(skillTrigger('$foo ', 5)).toBeNull()
    expect(skillTrigger('${HOME}', 2)).toBeNull()
    expect(skillTrigger('$?', 2)).toBeNull()
    expect(skillTrigger('$$', 2)).toBeNull()
    expect(skillTrigger('$a$b', 4)).toBeNull()
  })
})

describe('& prompt trigger', () => {
  test('fires at start and after whitespace; not mid-word; space and slash end the word', () => {
    expect(promptTrigger('&', 1)).toEqual({ start: 0, word: '' })
    expect(promptTrigger('&terse', 6)).toEqual({ start: 0, word: 'terse' })
    expect(promptTrigger('please &code_review', 19)).toEqual({ start: 7, word: 'code_review' })
    expect(promptTrigger('line 1\n&cite', 12)).toEqual({ start: 7, word: 'cite' })
    expect(promptTrigger('foo&bar', 7)).toBeNull()
    expect(promptTrigger('&terse ', 7)).toBeNull()
    expect(promptTrigger('&foo/bar', 8)).toBeNull()
    expect(promptTrigger('&terse', 0)).toBeNull()
  })

  test('inlineTrigger discriminates $ and &', () => {
    expect(inlineTrigger('$rev', 4)).toEqual({ kind: '$', start: 0, word: 'rev' })
    expect(inlineTrigger('&rev', 4)).toEqual({ kind: '&', start: 0, word: 'rev' })
    expect(inlineTrigger('@rev', 4)).toBeNull()
  })
})

describe('fenced code blocks suppress both triggers', () => {
  test('inside an unclosed ``` fence neither $ nor & opens; after closing ``` they do', () => {
    const insideSkill = '```sh\necho $HOME'
    expect(skillTrigger(insideSkill, insideSkill.length)).toBeNull()
    expect(inlineTrigger(insideSkill, insideSkill.length)).toBeNull()

    const insidePrompt = '```\n&terse'
    expect(promptTrigger(insidePrompt, insidePrompt.length)).toBeNull()
    expect(inlineTrigger(insidePrompt, insidePrompt.length)).toBeNull()

    const closedSkill = '```sh\necho $HOME\n```\n$review'
    expect(inlineTrigger(closedSkill, closedSkill.length)).toEqual({ kind: '$', start: 21, word: 'review' })

    const closedPrompt = '```\n&terse\n```\n&cite'
    expect(inlineTrigger(closedPrompt, closedPrompt.length)).toEqual({ kind: '&', start: 15, word: 'cite' })
  })
})

describe('stripTrigger and insertExpansion', () => {
  test('stripTrigger removes prefix + word + one trailing space', () => {
    expect(stripTrigger('hello $code', 6, 'code')).toBe('hello ')
    expect(stripTrigger('$code please', 0, 'code')).toBe('please')
    expect(stripTrigger('hello $code world', 6, 'code')).toBe('hello world')
    expect(stripTrigger('$', 0, '')).toBe('')
    expect(stripTrigger('$code  two', 0, 'code')).toBe(' two')
  })

  test('insertExpansion replaces token with trimmed text and returns end-of-insertion caret', () => {
    expect(insertExpansion('&terse', 0, 'terse', '  Be terse.\n')).toEqual({
      text: 'Be terse.',
      caret: 9,
    })
    expect(insertExpansion('Fix it. &terse', 8, 'terse', 'Be terse.')).toEqual({
      text: 'Fix it. Be terse.',
      caret: 17,
    })
    expect(insertExpansion('Fix it. &terse please', 8, 'terse', 'Be terse.')).toEqual({
      text: 'Fix it. Be terse. please',
      caret: 17,
    })
    // Non-whitespace after token gets a single separating space; caret lands at end of inserted text.
    expect(insertExpansion('&terse!now', 0, 'terse', 'Be terse.')).toEqual({
      text: 'Be terse. !now',
      caret: 9,
    })
    // Blank expansion strips the token like stripTrigger.
    expect(insertExpansion('hi &empty there', 3, 'empty', '   ')).toEqual({
      text: 'hi there',
      caret: 3,
    })
    expect(insertExpansion('&empty', 0, 'empty', '')).toEqual({
      text: '',
      caret: 0,
    })
  })
})

describe('filterSkillTriggers', () => {
  const skills = [
    skill('audit', 'Deep code review and 100 checks'),
    skill('code-review', 'Review pull requests'),
    skill('review-pr', 'Short PR review'),
    skill('v100', 'Release helper'),
  ]

  test('matches CODE_REVIEW to canonical code-review via case-insensitive _ ≡ -', () => {
    const hits = filterSkillTriggers(skills, 'CODE_REVIEW')
    expect(hits.map((s) => s.name)).toEqual(['code-review'])
  })

  test('ranks name prefix, then name substring, then description substring', () => {
    const hits = filterSkillTriggers(skills, 'review')
    expect(hits.map((s) => s.name)).toEqual(['review-pr', 'code-review', 'audit'])
  })

  test('digit-only query matches skill names only, not descriptions', () => {
    expect(filterSkillTriggers(skills, '100').map((s) => s.name)).toEqual(['v100'])
    expect(filterSkillTriggers([skill('audit', 'Top 100 checks')], '100')).toEqual([])
  })

  test('empty query returns all skills in index order', () => {
    expect(filterSkillTriggers(skills, '').map((s) => s.name)).toEqual(['audit', 'code-review', 'review-pr', 'v100'])
  })
})

describe('filterPromptTriggers', () => {
  const prompts = [
    prompt('1', 'My code review', 'Check everything'),
    prompt('2', 'Code review', 'Review for bugs and style'),
    prompt('3', 'Terse', 'Be brief.'),
  ]

  test('empty query returns all prompts in saved order', () => {
    expect(filterPromptTriggers(prompts, '').map((p) => p.id)).toEqual(['1', '2', '3'])
  })

  test('code_review, code-review, and Code match "Code review" as a prefix above substring matches', () => {
    expect(filterPromptTriggers(prompts, 'code_review').map((p) => p.id)).toEqual(['2', '1'])
    expect(filterPromptTriggers(prompts, 'code-review').map((p) => p.id)).toEqual(['2', '1'])
    expect(filterPromptTriggers(prompts, 'Code').map((p) => p.id)).toEqual(['2', '1'])
  })

  test('resolves duplicate names independently by id', () => {
    const dupes = [prompt('a', 'Brief', 'One sentence'), prompt('b', 'Brief', 'One bullet')]
    expect(filterPromptTriggers(dupes, 'brief').map((p) => p.id)).toEqual(['a', 'b'])
  })
})
