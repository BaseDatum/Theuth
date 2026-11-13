import type { ComponentType, ReactNode } from 'react'
import type { MethodMap } from './bindings/client'
import { supports, type Requirements } from './discovery'
import { useConnection } from '@/store/connection'

export function useSupported(required: Requirements): boolean {
  return useConnection(s => supports(s.discovery, required))
}

/** Existing queries remain targeted probes in explicit legacy mode, never an advertised feature catalog. */
export function useQuerySupport(methods: readonly (keyof MethodMap)[]): boolean {
  return useConnection(s => s.discovery.phase === 'legacy' || supports(s.discovery, { methods }))
}

export function Unavailable() {
  return <div role="status" className="m-auto p-6 text-sm text-fg-tertiary">Unavailable on this engine. Connect to an engine that supports this feature.</div>
}

export function Capability({ required, children, hide = false }: { required: Requirements; children: ReactNode; hide?: boolean }) {
  const available = useSupported(required)
  return available ? children : hide ? null : <Unavailable />
}

export function withCapability<P extends object>(Component: ComponentType<P>, required: Requirements) {
  return function Supported(props: P) {
    return <Capability required={required}><Component {...props} /></Capability>
  }
}
