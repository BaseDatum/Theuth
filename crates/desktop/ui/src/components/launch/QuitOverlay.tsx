import { CheckIcon, CircleCheckIcon, CircleIcon, Loader2Icon } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { create } from 'zustand'
import { isMacDesktop } from '@/lib/hotkeys'
import { DRAIN_MS, type QuitEvent, type QuitState, type QuitStep, isolate, quitSteps, quitTitle, reduce } from '@/lib/quit-progress'
import './QuitOverlay.css'

/** 163: the main window's quit state; `App.tsx` sends the request, the store changes and the shell's steps. */
export const useQuit = create<{ state: QuitState | null; send(e: QuitEvent): void }>()((set, get) => ({
  state: null,
  send: (e) => {
    const next = reduce(get().state, e)
    if (next !== get().state) set({ state: next })
  },
}))

const STATUS_TEXT: Record<QuitStep['status'], string> = { done: 'done', active: 'in progress', pending: 'waiting' }

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
      <div
        ref={box}
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="quit-title"
        tabIndex={-1}
        className="flex w-full max-w-[440px] flex-col gap-5 rounded-xl border border-border-subtle bg-elevated p-6 shadow-lg outline-none"
      >
        <div className="flex items-center gap-3">
          <Loader2Icon className="size-5 shrink-0 animate-spin text-fg-accent" aria-hidden />
          <h2 id="quit-title" className="text-base font-medium">
            {quitTitle(state)}
          </h2>
        </div>
        <ol aria-live="polite" className="flex flex-col gap-3">
          {quitSteps(state).map((step) => (
            <Row key={step.id} step={step} at={state.at} />
          ))}
        </ol>
      </div>
    </div>
  )
}

function Row({ step, at }: { step: QuitStep; at: number }) {
  return (
    <li className="flex gap-3">
      {step.status === 'done' ? (
        <CircleCheckIcon className="mt-0.5 size-4 shrink-0 text-success" aria-hidden />
      ) : step.status === 'active' ? (
        <Loader2Icon className="mt-0.5 size-4 shrink-0 animate-spin text-fg-accent" aria-hidden />
      ) : (
        <CircleIcon className="mt-0.5 size-4 shrink-0 text-fg-tertiary" aria-hidden />
      )}
      <div className="flex min-w-0 flex-1 flex-col gap-1.5">
        <p className={step.status === 'pending' ? 'text-sm text-fg-tertiary' : 'text-sm'}>
          {step.label}
          <span className="sr-only">, {STATUS_TEXT[step.status]}</span>
        </p>
        {step.detail && <p className="text-xs text-fg-secondary">{step.detail}</p>}
        {step.items && (
          <ul className="flex flex-col gap-1">
            {step.items.map((item) => (
              <li key={item.id} className="flex items-center gap-2 text-xs text-fg-secondary">
                {item.done ? (
                  <CheckIcon className="size-3 shrink-0 text-success" aria-hidden />
                ) : (
                  <span className="mx-[3px] size-1.5 shrink-0 animate-pulse rounded-full bg-fg-accent" aria-hidden />
                )}
                <span className="truncate">{item.label}</span>
                <span className="sr-only">{item.done ? ', saved' : ', saving'}</span>
              </li>
            ))}
          </ul>
        )}
        {step.bar && <DrainBar at={at} />}
      </div>
    </li>
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
