import { index, route, type RouteConfig } from '@react-router/dev/routes'

export default [
  index('routes/home.tsx'),
  route('docs/*', 'routes/docs.tsx'),
  // The search index, written to dist/api/search at build time and loaded by the static search client.
  route('api/search', 'routes/search.ts'),
  route('*', 'routes/not-found.tsx'),
] satisfies RouteConfig
