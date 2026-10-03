import {
  SearchDialog,
  SearchDialogClose,
  SearchDialogContent,
  SearchDialogHeader,
  SearchDialogIcon,
  SearchDialogInput,
  SearchDialogList,
  SearchDialogOverlay,
  type SharedProps,
} from 'fumadocs-ui/components/dialog/search'
import { type SearchClient, useDocsSearch } from 'fumadocs-core/search/client'
import { type AdvancedIndex, initAdvancedSearch, type SearchServer } from 'fumadocs-core/search/server'

/** The search database, built once per page load from the records at /api/search (routes/search.ts). No server. */
let db: Promise<SearchServer> | undefined
const client: SearchClient = {
  async search(query) {
    db ??= fetch('/api/search')
      .then((r) => r.json() as Promise<AdvancedIndex[]>)
      .then((indexes) => initAdvancedSearch({ indexes, language: 'english', sort: { enabled: false } }))
    return (await db).search(query)
  },
}

/** ⌘K / Ctrl+K dialog; heading-level results with breadcrumbs. */
export default function DocsSearchDialog(props: SharedProps) {
  const { search, setSearch, query } = useDocsSearch({ client })

  return (
    <SearchDialog search={search} onSearchChange={setSearch} isLoading={query.isLoading} {...props}>
      <SearchDialogOverlay />
      <SearchDialogContent>
        <SearchDialogHeader>
          <SearchDialogIcon />
          <SearchDialogInput />
          <SearchDialogClose />
        </SearchDialogHeader>
        <SearchDialogList items={query.data !== 'empty' ? query.data : null} />
      </SearchDialogContent>
    </SearchDialog>
  )
}
