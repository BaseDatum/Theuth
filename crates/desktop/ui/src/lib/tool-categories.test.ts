import { expect, test } from 'vitest'
import type { ToolCategoryInfo } from '@/rpc/bindings/method.agent.output'
import { effectiveCategoryHidden, toolCategoryKey } from './tool-categories'

const category = (over: Partial<ToolCategoryInfo> = {}): ToolCategoryInfo => ({
  id: 'engineering',
  label: 'Engineering',
  description: 'Files and shell',
  custom: false,
  default_enabled: true,
  hidden: false,
  builtin_count: 2,
  builtin_tools: ['read_file', 'shell'],
  mcp_servers: [],
  ...over,
})

test('toolCategoryKey uses the session hide prefix', () => {
  expect(toolCategoryKey('web')).toBe('tcat:web')
})

test('a live session uses the engine category hidden flag, not draft or default state', () => {
  expect(effectiveCategoryHidden(category({ hidden: true, default_enabled: false }), 's1', { 'tcat:engineering': false })).toBe(true)
  expect(effectiveCategoryHidden(category({ hidden: false }), 's1', { 'tcat:engineering': true })).toBe(false)
})

test('a new-chat draft falls back to the category default', () => {
  expect(effectiveCategoryHidden(category(), '')).toBe(false)
  expect(effectiveCategoryHidden(category({ default_enabled: false }), '')).toBe(true)
})

test('new-chat draft entries override defaults in both directions', () => {
  expect(effectiveCategoryHidden(category(), '', { 'tcat:engineering': true })).toBe(true)
  expect(effectiveCategoryHidden(category({ default_enabled: false }), '', { 'tcat:engineering': false })).toBe(false)
})

test('a draft entry for another category does not affect the fallback', () => {
  expect(effectiveCategoryHidden(category({ default_enabled: false }), '', { 'tcat:web': false })).toBe(true)
})
