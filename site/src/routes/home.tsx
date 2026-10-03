import { useSyncExternalStore } from 'react'
import { App } from '../App'

const noop = () => () => {}

/**
 * `/` is prerendered only for its shell (document, preloads, inline route modules): App.tsx reads `navigator` while
 * rendering, so it mounts after hydration exactly as the old CSR page did. Server and first client render both yield
 * null, which keeps hydration clean.
 */
export default function Home() {
  const client = useSyncExternalStore(
    noop,
    () => true,
    () => false,
  )
  return client ? <App /> : null
}
