import { Outlet, createFileRoute } from '@tanstack/react-router'
import { Explorer } from '@/components/artifacts/Explorer'
import { isPopout } from '@/lib/popout'
import type { Kind } from "@/rpc/bindings/method.artifact.output"

const KINDS: Record<Kind, true> = { markdown: true, text: true, csv: true, image: true, svg: true, pdf: true, html: true, file: true, app: true, archive: true, mermaid: true }

/** `?q=`, `?kind=`, `?session=` prefill the rail's filters. A popout window shows the viewer alone, no rail. */
export const Route = createFileRoute('/artifacts')({
  validateSearch: (s: Record<string, unknown>): { q?: string; kind?: Kind; session?: string } => {
    const out: { q?: string; kind?: Kind; session?: string } = {}
    if (typeof s.q === 'string' && s.q !== '') out.q = s.q
    if (typeof s.kind === 'string' && s.kind in KINDS) out.kind = s.kind as Kind
    if (typeof s.session === 'string' && s.session !== '') out.session = s.session
    return out
  },
  component: isPopout() ? Outlet : Explorer,
})
