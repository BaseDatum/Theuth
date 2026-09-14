import { Link } from '@tanstack/react-router'
import type { ReactNode } from 'react'

/** Concepts that link here (`Concept.cited_by`). */
export function Backlinks({ ids }: { ids: string[] }) {
  return (
    <Rail title="Cited by" empty="Nothing links here yet">
      {ids.map((id) => (
        <li key={id}>
          <Link
            to="/knowledge/$"
            params={{ _splat: id }}
            className="block truncate rounded-md px-2 py-1 text-sm hover:bg-layer-2"
          >
            {id}
          </Link>
        </li>
      ))}
    </Rail>
  )
}

/** Outbound links (`Concept.links`); missing targets are marked and create on click. */
export function Links({ links, onMissing }: { links: [string, boolean][]; onMissing: (id: string) => void }) {
  return (
    <Rail title="Links" empty="No links">
      {links.map(([id, exists]) => (
        <li key={id}>
          {exists ? (
            <Link
              to="/knowledge/$"
              params={{ _splat: id }}
              className="block truncate rounded-md px-2 py-1 text-sm hover:bg-layer-2"
            >
              {id}
            </Link>
          ) : (
            <button
              type="button"
              className="flex w-full items-center gap-1 rounded-md px-2 py-1 text-left text-sm text-danger hover:bg-layer-2"
              title="Create this document"
              onClick={() => onMissing(id)}
            >
              <span className="truncate">{id}</span>
              <span className="shrink-0 text-xs">→ missing</span>
            </button>
          )}
        </li>
      ))}
    </Rail>
  )
}

function Rail({ title, empty, children }: { title: string; empty: string; children: ReactNode[] }) {
  return (
    <section className="px-2 py-3">
      <h3 className="px-2 pb-1 text-xs font-semibold text-fg-tertiary uppercase">{title}</h3>
      {children.length === 0 ? <p className="px-2 text-sm text-fg-tertiary">{empty}</p> : <ul>{children}</ul>}
    </section>
  )
}
