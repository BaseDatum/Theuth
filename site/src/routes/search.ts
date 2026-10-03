import type { AdvancedIndex } from 'fumadocs-core/search/server'
import { source } from '../lib/source'

/**
 * Prerendered (react-router.config.ts) to dist/api/search: one record per page with its structured content.
 * The browser builds the ZBSearch database from this (components/search.tsx); shipping the prebuilt database
 * instead (`staticGET()`) was 4 MB for 30 pages and the relay serves files uncompressed.
 */
export async function loader() {
  const indexes: AdvancedIndex[] = await Promise.all(
    source.getPages().map(async (page) => ({
      id: page.url,
      url: page.url,
      title: page.data.title,
      description: page.data.description,
      structuredData: await page.data.structuredData(),
    })),
  )
  return Response.json(indexes)
}
