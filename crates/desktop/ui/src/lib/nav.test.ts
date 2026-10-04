import { expect, test } from 'vitest'
import { sectionOf, tabOf } from './nav'

test('tabOf matches the route exactly or as a prefix at a / boundary', () => {
  expect(tabOf('agents', '/agents')).toBe('/agents')
  expect(tabOf('agents', '/agents/new')).toBe('/agents')
  expect(tabOf('agents', '/tasks/abc')).toBe('/tasks')
  expect(tabOf('agents', '/routines/x/overview')).toBe('/routines')
  expect(tabOf('library', '/meetings/briefs')).toBe('/meetings')
  expect(tabOf('library', '/meetingsx')).toBeNull()
  expect(tabOf('library', '/agents')).toBeNull()
  expect(tabOf('agents', '/artifacts')).toBeNull()
})

test('sectionOf lights the right sidebar row', () => {
  expect(sectionOf('/')).toBe('home')
  expect(sectionOf('/talk/abc')).toBe('talk')
  expect(sectionOf('/agents')).toBe('agents')
  expect(sectionOf('/tasks/abc')).toBe('agents')
  expect(sectionOf('/routines/new')).toBe('agents')
  expect(sectionOf('/attention')).toBe('attention')
  expect(sectionOf('/artifacts')).toBe('library')
  expect(sectionOf('/knowledge/a/b.md')).toBe('library')
  expect(sectionOf('/meetings')).toBe('library')
  expect(sectionOf('/meetings/briefs')).toBe('library')
  expect(sectionOf('/meetings/abc')).toBe('library')
  expect(sectionOf('/tools')).toBe('library')
  expect(sectionOf('/meetings/new')).toBe('meeting')
  expect(sectionOf('/c/abc')).toBeNull()
  expect(sectionOf('/settings/general')).toBeNull()
  expect(sectionOf('/new')).toBeNull()
})
