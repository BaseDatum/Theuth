// Whether the WebGPU orb can actually render. `navigator.gpu` alone is not enough: the iOS Simulator exposes it
// but `requestAdapter()` gives nothing, and the shader canvas stays blank. Probed once per session; `null` while
// pending. A runtime failure (`webGpuFailed`) turns it off for the rest of the session.

import { useSyncExternalStore } from 'react'

let usable: boolean | null = null
let probe: Promise<void> | undefined
const listeners = new Set<() => void>()

const set = (next: boolean) => {
  usable = next
  for (const listener of listeners) listener()
}

const probeOnce = () => {
  probe ??= (async () => ('gpu' in navigator ? (await navigator.gpu.requestAdapter()) != null : false))().then(set, () =>
    set(false),
  )
}

const subscribe = (listener: () => void) => {
  listeners.add(listener)
  probeOnce()
  return () => void listeners.delete(listener)
}

const snapshot = () => usable

export const useWebGpu = (): boolean | null => useSyncExternalStore(subscribe, snapshot, snapshot)

export const webGpuFailed = () => set(false)
