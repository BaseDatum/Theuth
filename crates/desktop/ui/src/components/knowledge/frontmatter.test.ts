import { describe, expect, it } from 'vitest'
import { checkId, scaffold, stripFrontmatter } from './frontmatter'

describe('stripFrontmatter', () => {
  it('splits a leading block from the body', () => {
    expect(stripFrontmatter('---\ntype: fact\ntitle: T\n---\n\n# Body\n')).toEqual({
      frontmatter: 'type: fact\ntitle: T',
      body: '\n# Body\n',
    })
  })

  it('returns the whole text when there is no block', () => {
    expect(stripFrontmatter('# Body\n\n---\n\nmore')).toEqual({ frontmatter: null, body: '# Body\n\n---\n\nmore' })
  })

  it('leaves a --- rule inside the body untouched', () => {
    const { body } = stripFrontmatter('---\ntype: fact\n---\nabove\n\n---\n\nbelow')
    expect(body).toBe('above\n\n---\n\nbelow')
  })

  it('accepts an empty block and a block at end of file', () => {
    expect(stripFrontmatter('---\n---\nx')).toEqual({ frontmatter: '', body: 'x' })
    expect(stripFrontmatter('---\ntype: fact\n---')).toEqual({ frontmatter: 'type: fact', body: '' })
  })

  it('ignores a first line that is not exactly ---', () => {
    expect(stripFrontmatter('----\nx\n---\n').frontmatter).toBeNull()
  })
})

describe('scaffold', () => {
  it('types the concept by its first segment', () => {
    expect(scaffold('facts/theuth', 'Theuth')).toBe('---\ntype: facts\ntitle: Theuth\ndescription: \ntags: []\n---\n\n')
    expect(scaffold('solo', 'S')).toContain('type: solo\n')
  })
})

describe('checkId', () => {
  it('lowercases and reports it', () => {
    expect(checkId('Facts/THEUTH')).toEqual({ id: 'facts/theuth', lowered: true, error: null })
    expect(checkId('facts/theuth').lowered).toBe(false)
  })

  it('rejects too many segments, bad characters and reserved names', () => {
    expect(checkId('a/b/c/d').error).toMatch(/3 segments/)
    expect(checkId('a/-b').error).toMatch(/start with/)
    expect(checkId('a//b').error).not.toBeNull()
    expect(checkId('index').error).toMatch(/reserved/)
    expect(checkId('log').error).toMatch(/reserved/)
    expect(checkId('facts/index').error).toBeNull()
  })

  it('treats empty input as not yet an error', () => {
    expect(checkId('  ')).toEqual({ id: '', lowered: false, error: null })
  })
})
