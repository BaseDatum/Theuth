import { withCapability } from '@/rpc/capabilities'
import { requirements } from '@/components/artifacts/requirements'
import { Outlet, createFileRoute } from '@tanstack/react-router'
import { Explorer } from '@/components/artifacts/Explorer'
import { POPOUT, artifactsRail } from '@/lib/popout'
import type { Kind } from "@/rpc/bindings/method.artifact.output"

const KINDS: Record<Kind, true> = { markdown: true, text: true, csv: true, image: true, svg: true, pdf: true, html: true, file: true, app: true, archive: true, mermaid: true }

/**
 * `?q=`, `?kind=`, `?session=` prefill the rail's filters. An artifact window shows the viewer alone, no rail; every
 * other window — the sidebar's Library page window included — gets the Explorer (`artifactsRail`).
 */
export const Route = createFileRoute('/artifacts')({
  validateSearch: (s: Record<string, unknown>): { q?: string; kind?: Kind; session?: string } => {
    const out: { q?: string; kind?: Kind; session?: string } = {}
    if (typeof s.q === 'string' && s.q !== '') out.q = s.q
    if (typeof s.kind === 'string' && s.kind in KINDS) out.kind = s.kind as Kind
    if (typeof s.session === 'string' && s.session !== '') out.session = s.session
    return out
  },
  component: withCapability(artifactsRail(POPOUT) ? Explorer : Outlet, requirements),
})
