// `-` prefix: the router plugin skips it (not a route), vitest still runs it.
import '../components/settings/test-dom'
import { type AnyRoute, type AnyRouter, createMemoryHistory, createRootRoute, createRoute, createRouter } from '@tanstack/react-router'
import { expect, test } from 'vitest'
import { Route as Index } from './index'

/** The real `/` route's options in a tree beside a stand-in `/new`. */
async function land(entry: string) {
  const root = createRootRoute()
  const index = createRoute({ getParentRoute: () => root, path: '/', validateSearch: Index.options.validateSearch as never, beforeLoad: Index.options.beforeLoad as never })
  const fresh = createRoute({ getParentRoute: () => root, path: '/new', validateSearch: (s: Record<string, unknown>) => s })
  const history = createMemoryHistory({ initialEntries: [entry] })
  // A throwaway tree, not the app's registered one: typed loosely.
  const router: AnyRouter = createRouter({ routeTree: root.addChildren([index as unknown as AnyRoute, fresh as unknown as AnyRoute]), history } as never)
  await router.load()
  return { location: router.state.location, history }
}

test('/ redirects to the new-chat page, replacing the entry (Back does not bounce through /)', async () => {
  const { location, history } = await land('/')
  expect(location.pathname).toBe('/new')
  expect(location.search).toEqual({})
  expect(history.length).toBe(1)
})

test('/?project= keeps its project on the way to /new', async () => {
  const { location } = await land('/?project=theuth')
  expect(location.pathname).toBe('/new')
  expect(location.search).toEqual({ project: 'theuth' })
})
