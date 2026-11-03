// Design 164 D3: a fence drawn off-screen for a channel picture (`lib/fence-snapshot.ts`). The context's default is
// `null` — the chat, artifact viewers and every other mount read nothing from it and render exactly as before; only the
// snapshot host provides a value, and the few readers (diagram tokens, DiagramFrame/OpenUI chrome, the mermaid
// palette, `FenceFallback`) change only then.

import { createContext, useContext, useMemo, useSyncExternalStore } from 'react'
import { type DiagramTokens, diagramTokens } from './diagram-theme'
import type { Appearance } from './page-theme'
import { subscribeDark } from './theme'

export type SnapshotMode = {
  /** The palette the picture is drawn under, whatever the app shows. */
  appearance: Appearance
  /** The frame width in CSS px. */
  width: number
  /** The off-screen frame: it carries `appearance`'s theme variables, so tokens resolve off it. */
  root: HTMLElement
  /** The fence could not be drawn (`FenceFallback` mounted): the snapshot answers `failed` with `reason`. */
  fail: (reason: string) => void
}

export const SnapshotContext = createContext<SnapshotMode | null>(null)

export const useSnapshot = (): SnapshotMode | null => useContext(SnapshotContext)

const isDark = () => document.documentElement.classList.contains('dark')

/** The diagram tokens: the app theme's (re-read when `.dark` flips), or the snapshot frame's forced palette. */
export function useThemeTokens(): DiagramTokens {
  const snap = useSnapshot()
  const dark = useSyncExternalStore(subscribeDark, isDark, () => false)
  // `dark` is the re-read trigger: the tokens resolve off the live stylesheet.
  return useMemo(() => diagramTokens(snap?.root), [dark, snap])
}
