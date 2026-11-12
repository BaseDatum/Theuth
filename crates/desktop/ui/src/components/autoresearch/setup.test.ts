import { expect, test, vi } from 'vitest'
import type { CreateSessionOptions } from '@/rpc/bindings/method.sessionsync.input'
import { type Entry, useSessions } from '@/store/sessions'
import { CARD_PRESS, autoOpens, discardMessage, editMessage, lastSetupResultId, latestStudyDraft, leafValue, recentRepos, seedText, sessionTitle, sparkPoints, startSetupChat } from './setup'
import { studies } from './fixtures'

const tool = (callId: string, name: string, args: unknown, result?: boolean): Entry => ({ id: callId, kind: 'tool', callId, name, args, pending: false, child: false, ...(result ? { result: { output: 'ok', isError: false, files: [] } } : {}) })

test('startSetupChat: a root with cwd = the repo, its title, the slice seeded, the page told, then the skill with the §4.6 seed (151 §2 step 3)', async () => {
  const calls: string[] = []
  const api = {
    create_session: vi.fn(async (o: CreateSessionOptions) => {
      calls.push(`create_session cwd=${o.cwd} retention=${o.retention}`)
      return { session: 's1', root: 's1', retention: o.retention ?? 'durable', lease_secret: null }
    }),
    rename_session: vi.fn(async (id: string, title: string) => {
      calls.push(`rename_session ${id} ${title}`)
      return null
    }),
    use_skill: vi.fn(async (id: string | null, name: string, text: string) => {
      calls.push(`use_skill ${id} ${name} ${text}`)
      return id ?? ''
    }),
  }
  const loaded = vi.spyOn(useSessions.getState(), 'loadHistory').mockImplementation(() => {
    calls.push('loadHistory')
    return true
  })
  const id = await startSetupChat(api, '/Users/sky/Develop/theuth', 'start faster', (s) => calls.push(`onSession ${s}`))
  expect(id).toBe('s1')
  expect(calls).toEqual(['create_session cwd=/Users/sky/Develop/theuth retention=durable', 'rename_session s1 New study: start faster', 'loadHistory', 'onSession s1', 'use_skill s1 autoresearch [autoresearch setup · repo /Users/sky/Develop/theuth]\nstart faster'])
  loaded.mockRestore()
})

test('seed and title fall back to the repo basename and the help line without a goal', () => {
  expect(seedText('/a/b/theuth/', '  ')).toBe('[autoresearch setup · repo /a/b/theuth/]\nHelp me set up a study for this repository')
  expect(sessionTitle('/a/b/theuth/', '')).toBe('New study: theuth')
  expect(editMessage('tui-startup', 'budget.usd', '30')).toBe('[study draft tui-startup] you set budget.usd to 30 in the panel')
  expect(discardMessage('tui-startup')).toBe('[study draft tui-startup] discarded in the panel')
  expect(CARD_PRESS.exec('study draft tui-startup: open in autoresearch')?.[1]).toBe('tui-startup')
  expect(CARD_PRESS.test('study draft Tui: open in autoresearch')).toBe(false)
})

test('latestStudyDraft picks the newest autoresearch_draft call with a slug; pending calls without args are skipped', () => {
  const entries = [tool('c1', 'autoresearch_draft', { slug: 'a', goal: 'x' }, true), tool('c2', 'autoresearch_probe', { draft_id: 'a', what: 'bench' }, true), tool('c3', 'autoresearch_draft', { slug: 'b' }), tool('c4', 'autoresearch_draft', undefined)]
  expect(latestStudyDraft(entries)).toEqual({ draftId: 'b', callId: 'c3' })
  expect(latestStudyDraft([{ id: 'u', kind: 'user', text: 'hi' }])).toBeNull()
})

test('lastSetupResultId changes on a new draft or probe result, not on a call that is still running', () => {
  const entries = [tool('c1', 'autoresearch_draft', { slug: 'a' }, true)]
  expect(lastSetupResultId(entries)).toBe('c1')
  entries.push(tool('c2', 'autoresearch_probe', { draft_id: 'a', what: 'bench' }))
  expect(lastSetupResultId(entries)).toBe('c1')
  entries[1] = tool('c2', 'autoresearch_probe', { draft_id: 'a', what: 'bench' }, true)
  expect(lastSetupResultId(entries)).toBe('c2')
  entries.push(tool('c3', 'bash', { command: 'ls' }, true))
  expect(lastSetupResultId(entries)).toBe('c2')
})

test('recentRepos: distinct repos of studies ∪ drafts, newest first', () => {
  const s = (repo: string, started_at: string | null) => ({ repo, started_at })
  const d = (repo: string, updated_at: string) => ({ repo, updated_at })
  expect(recentRepos([s('/old', '2026-01-01T00:00:00Z'), s('/mid', '2026-05-01T00:00:00Z'), s('/old', '2026-02-01T00:00:00Z')], [d('/new', '2026-10-01T00:00:00Z'), d('/mid', '2026-03-01T00:00:00Z'), d('', '2026-10-02T00:00:00Z')])).toEqual(['/new', '/mid', '/old'])
  expect(recentRepos([], [])).toEqual([])
})

test('leafValue renders the §4.3 grammar: nested leaves, lists joined, guards one per line, absent = empty', () => {
  const spec = { ...studies()[0]!.detail.spec, secondary_guards: [{ name: 'rss_mb', max_regression: '5%', direction: 'lower' as const }], holdout: null, window: null }
  expect(leafValue(spec, 'metric.direction')).toBe(spec.metric.direction)
  expect(leafValue(spec, 'scope')).toBe(spec.scope.join(', '))
  expect(leafValue(spec, 'secondary_guards')).toBe('rss_mb 5% lower')
  expect(leafValue(spec, 'holdout.command')).toBe('')
  expect(leafValue(spec, 'window')).toBe('')
  expect(leafValue(spec, 'bench.repeats')).toBe(String(spec.bench.repeats))
})

test('sparkPoints: one point per sample scaled into the box; null below two values', () => {
  expect(sparkPoints([1])).toBeNull()
  expect(sparkPoints([0, 10], 56, 12)).toBe('0.0,10.5 56.0,1.5')
  expect(sparkPoints([5, 5, 5])?.split(' ')).toHaveLength(3)
})

test('autoOpens: the study-drafts card never slides the artifact sheet over the setup pane; elsewhere, and every other app, it does (live proof Z)', () => {
  // The engine names the artifact `<app>.openui` (seen live: `study-drafts.openui`).
  expect(autoOpens('study-drafts.openui', '/autoresearch/new')).toBe(false)
  expect(autoOpens('study-drafts.openui', '/autoresearch/new?chat=s1')).toBe(false)
  expect(autoOpens('study-drafts.openui', '/c/s1')).toBe(true)
  expect(autoOpens('sort-study-setup.openui', '/autoresearch/new')).toBe(true)
})
