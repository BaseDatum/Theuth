import { useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { create } from 'zustand'
import { isMacDesktop } from '@/lib/hotkeys'
import { DRAIN_MS, type QuitEvent, type QuitState, isolate, quitSteps, quitTitle, reduce } from '@/lib/quit-progress'
import { LaunchCard, StepList, StepRow } from './LaunchCard'
import './QuitOverlay.css'

/** 163: the main window's quit state; `App.tsx` sends the request, the store changes and the shell's steps. */
export const useQuit = create<{ state: QuitState | null; send(e: QuitEvent): void }>()((set, get) => ({
  state: null,
  send: (e) => {
    const next = reduce(get().state, e)
    if (next !== get().state) set({ state: next })
  },
}))

/**
 * 163 D5: full-window and non-dismissable — no buttons, nothing behind it reachable. A direct child of `<body>`
 * above every layer (Radix portals sit at `z-50`, the toaster higher), every sibling inert, focus on the dialog.
 * What the app is doing until the process is gone: the chats the engine is saving, the companion, the engine, closing.
 */
export function QuitOverlay() {
  const state = useQuit((s) => s.state)
  return state && createPortal(<Overlay state={state} />, document.body)
}

const bodyChildren = (): HTMLElement[] => [...document.body.children].filter((el) => el instanceof HTMLElement)

function Overlay({ state }: { state: QuitState }) {
  const host = useRef<HTMLDivElement>(null)
  const box = useRef<HTMLDivElement>(null)
  useEffect(() => {
    const keep = host.current
    if (!keep) return
    const changed = isolate(bodyChildren(), keep)
    // A portal that opens after the overlay (a dialog some event raised) is made inert as it lands.
    const watch = new MutationObserver(() => changed.push(...isolate(bodyChildren(), keep)))
    watch.observe(document.body, { childList: true })
    box.current?.focus()
    return () => {
      watch.disconnect()
      for (const el of changed) el.inert = false
    }
  }, [])
  return (
    <div
      ref={host}
      className="fixed inset-0 z-[2147483647] flex items-center justify-center bg-background/90 p-4 text-foreground backdrop-blur-sm"
      data-tauri-drag-region={isMacDesktop() || undefined}
    >
      <LaunchCard
        ref={box}
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="quit-title"
        tabIndex={-1}
        className="outline-none"
        title={quitTitle(state)}
        titleId="quit-title"
      >
        <StepList>
          {quitSteps(state).map((step) => (
            <StepRow key={step.id} step={step}>
              {step.bar && <DrainBar at={state.at} />}
            </StepRow>
          ))}
        </StepList>
      </LaunchCard>
    </div>
  )
}

/** The drain deadline (`DRAIN_MS` nominal): a CSS fill from the request, so it keeps time across re-renders. */
function DrainBar({ at }: { at: number }) {
  const [delay] = useState(() => at - Date.now())
  return (
    <div className="mt-0.5 h-1 overflow-hidden rounded-full bg-layer-2" aria-hidden>
      <div className="quit-drain h-full bg-fg-accent" style={{ animationDuration: `${DRAIN_MS}ms`, animationDelay: `${delay}ms` }} />
    </div>
  )
}
