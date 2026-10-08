// Local mirror of 144 §13.1 until the contract worker's ts-rs bindings land
// (`…/144-briefs/bindings/.bindings-ready`). Then: copy the bindings over, switch imports to
// `@/rpc/bindings/...`, delete this file. Names and shapes match the Rust types exactly; i64/u64 → bigint
// as ts-rs emits them.
import type { DocId } from '@/rpc/bindings/DocId'
import type { DocSummary } from '@/rpc/bindings/DocSummary'
import type { LocateStatus } from '@/rpc/bindings/LocateStatus'

export type ItemKind = 'source' | 'note' | 'session' | 'artifact'

export type NotebookItem = {
  notebook: DocId
  kind: ItemKind
  item: string
  added_ms: bigint
  added_by: string
  removed_ms: bigint | null
}

export type Notebook = {
  doc: DocSummary
  project: string | null
  archived_ms: bigint | null
  sources: number
  notes: number
  chats: number
}

export type McpLabel = 'confluence' | 'jira' | 'other'
export type YoutubeRoute = 'captions' | 'audio'

export type SourceOrigin =
  | { kind: 'file'; name: string; sha256: string; ext: string }
  | { kind: 'url'; url: string; canonical: string | null }
  | { kind: 'meeting'; meeting: string }
  | { kind: 'text'; title: string; sha256: string }
  | { kind: 'session'; session: string }
  | { kind: 'mcp'; server: string; tool: string; args: unknown; label: McpLabel }
  | { kind: 'youtube'; video: string; url: string; route: YoutubeRoute }
  | { kind: 'audio'; name: string; sha256: string; ext: string }
  | { kind: 'video'; name: string; sha256: string; ext: string }

export type SourceStatus = 'new' | 'queued' | 'running' | 'completed' | 'failed'

export type Source = {
  doc: DocSummary
  origin: SourceOrigin
  status: SourceStatus
  progress: number | null
  error: string | null
  attempts: number
  fetched_ms: bigint | null
  content_sha: string | null
  changed_since: bigint | null
  bytes: bigint | null
  words: number | null
  duration_ms: bigint | null
  pages: number | null
  insights: number
  meta: unknown
}

export type ContextLevel = 'off' | 'mention' | 'summary' | 'full'

export type ContextItem = {
  kind: ItemKind
  item: string
  level: ContextLevel
  status: 'available' | 'truncated' | 'omitted'
  tokens: number
}

export type ContextEstimate = { items: ContextItem[]; tokens: number; budget: number; omitted: number }

export type CiteKind = 'notes' | 'sources' | 'knowledge' | 'meetings' | 'sessions'

export type Citation = {
  href: string
  kind: CiteKind
  id: string
  locator: string | null
  sha8: string | null
  status: LocateStatus
  available: boolean
  title: string
  text: string | null
}

export type SourceEvent =
  | { type: 'changed'; source: DocId; status: SourceStatus }
  | { type: 'progress'; source: DocId; percent: number }
