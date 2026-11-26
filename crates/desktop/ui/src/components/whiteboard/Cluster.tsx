// 218 §8.8 "Control cluster" / design §4.2: the vertical glass column above the composer. Icons with accessible names,
// no visible text, 44 pt targets; order as the mock: ink tools · Add · Link · Frame · Select | Fit · Outline · Undo ·
// Redo · ⋯. Every interaction on it restarts the hide timer (`onActivity`). The composer's tools button opens the same
// ink panel and actions in one menu (`ToolsMenu`).

import {
  Clipboard,
  Crosshair,
  Ellipsis,
  Eraser,
  FileText,
  FileCode,
  Frame,
  GitCompare,
  Globe,
  Hand,
  Highlighter,
  Image as ImageIcon,
  LassoSelect,
  LayoutDashboard,
  ListTodo,
  ListTree,
  Pen,
  Plus,
  Presentation,
  Redo2,
  Scan,
  Spline,
  SquareTerminal,
  StickyNote,
  Undo2,
} from 'lucide-react'
import { type ComponentProps, type ReactNode, useEffect, useState } from 'react'
import { useStore } from 'zustand'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { usePointerCoarse } from '@/lib/mobile'
import { useDark } from '@/lib/theme'
import type { InkColour, InkTool } from './canvas/model'
import { pickInkTool } from './canvas/input'
import type { Mode, WbStore } from './canvas/store'
import { INK_COLOURS, inkCss } from './ink/colour'

export type AddKind = 'sticky' | 'note' | 'artifact' | 'excerpt' | 'diff' | 'terminal' | 'web' | 'image' | 'view' | 'tasks' | 'clipboard'

export type ClusterProps = {
  store: WbStore
  onActivity(): void
  onAdd(kind: AddKind): void
  onFit(): void
  onOutline(): void
  /** Present mode (`present.ts`, also Shift+P): every piece of chrome hidden, Hand locked; Esc leaves it. */
  onPresent(): void
  onUndo(): void
  onRedo(): void
  canUndo: boolean
  canRedo: boolean
  /** The ⋯ menu's body (title, background, sessions, archived, toggles, close). */
  more: ReactNode
  /** What Add can offer: kinds that need a bound session are off without one. */
  addable: Record<AddKind, boolean>
  /** A popover of the cluster opened (true) or every one closed (false): the auto-hide waits while one is open. */
  onHold(open: boolean): void
}

const TOOLS: { tool: InkTool; label: string; icon: ReactNode }[] = [
  { tool: 'pen', label: 'Pen', icon: <Pen className="size-5" /> },
  { tool: 'monoline', label: 'Fine liner', icon: <Spline className="size-5" /> },
  { tool: 'marker', label: 'Marker', icon: <Highlighter className="size-5" /> },
]
export const WIDTH: Record<InkTool, number> = { monoline: 2.4, pen: 2.8, marker: 6 }
/** The width choices, as factors of the tool's own width. */
const SIZES: { label: string; k: number; dot: number }[] = [
  { label: 'Thin', k: 0.6, dot: 4 },
  { label: 'Medium', k: 1, dot: 7 },
  { label: 'Thick', k: 1.8, dot: 11 },
]

/** What the tools buttons show: the active tool (Hand included), its name, and whether it is armed. */
export function toolFace(mode: Mode, tool: InkTool): { label: string; icon: ReactNode; armed: boolean } {
  if (mode === 'hand') return { label: 'Hand', icon: <Hand className="size-5" />, armed: true }
  if (mode === 'erase') return { label: 'Eraser', icon: <Eraser className="size-5" />, armed: true }
  if (mode === 'select') return { label: 'Select', icon: <LassoSelect className="size-5" />, armed: true }
  if (mode === 'link') return { label: 'Link', icon: <Spline className="size-5 -scale-x-100" />, armed: true }
  if (mode === 'frame') return { label: 'Frame', icon: <Frame className="size-5" />, armed: true }
  const t = TOOLS.find((x) => x.tool === tool) ?? (TOOLS[0] as (typeof TOOLS)[number])
  return { label: t.label, icon: t.icon, armed: mode === 'draw' }
}

const ADD: { kind: AddKind; label: string; icon: ReactNode }[] = [
  { kind: 'sticky', label: 'Sticky', icon: <StickyNote className="size-4" /> },
  { kind: 'note', label: 'Note', icon: <FileText className="size-4" /> },
  { kind: 'artifact', label: 'Artifact from the session', icon: <Presentation className="size-4" /> },
  { kind: 'excerpt', label: 'File excerpt', icon: <FileCode className="size-4" /> },
  { kind: 'diff', label: 'Diff of the working tree', icon: <GitCompare className="size-4" /> },
  { kind: 'terminal', label: 'Terminal', icon: <SquareTerminal className="size-4" /> },
  { kind: 'web', label: 'Web page', icon: <Globe className="size-4" /> },
  { kind: 'image', label: 'Image', icon: <ImageIcon className="size-4" /> },
  { kind: 'view', label: 'App view…', icon: <LayoutDashboard className="size-4" /> },
  { kind: 'tasks', label: 'Tasks…', icon: <ListTodo className="size-4" /> },
  { kind: 'clipboard', label: 'From clipboard', icon: <Clipboard className="size-4" /> },
]

/** Spreads the rest (ref, aria-expanded, data-state, handlers) so it can be a Radix `PopoverTrigger asChild`: without
 *  the ref the popover has no anchor and is placed off screen. */
export function ClusterButton({ label, on, children, ...rest }: { label: string; on?: boolean; children: ReactNode } & Omit<ComponentProps<'button'>, 'children'>) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      aria-pressed={on === undefined ? undefined : on}
      {...rest}
      className="wb-press grid size-11 place-items-center rounded-[14px] disabled:opacity-35 focus-visible:outline focus-visible:outline-2 focus-visible:outline-fg-accent"
    >
      {children}
    </button>
  )
}

const Sep = () => <div className="my-0.5 h-px w-7 bg-current opacity-15" aria-hidden />

/** The ink panel: Hand · Pen · Fine liner · Marker · Eraser, the colours and the width. Every pick latches until
 *  another pick, Hand, or Esc. */
export function InkPanel({ store, onActivity }: { store: WbStore; onActivity(): void }) {
  const tool = useStore(store, (s) => s.tool)
  const colour = useStore(store, (s) => s.colour)
  const mode = useStore(store, (s) => s.mode)
  const width = useStore(store, (s) => s.width)
  const dark = useDark()
  const pick = (patch: Partial<{ tool: InkTool; colour: InkColour; mode: Mode; width: number }>) => {
    store.getState().set(patch)
    onActivity()
  }
  const k = width / WIDTH[tool]
  const size = SIZES.reduce((a, b) => (Math.abs(b.k - k) < Math.abs(a.k - k) ? b : a))
  return (
    <div className="flex flex-col gap-2" role="group" aria-label="Ink">
      <div className="flex gap-1">
        <ClusterButton label="Hand" on={mode === 'hand'} onClick={() => pick({ mode: 'hand' })}>
          <Hand className="size-5" />
        </ClusterButton>
        {TOOLS.map((t) => (
          <ClusterButton key={t.tool} label={t.label} on={mode === 'draw' && tool === t.tool} onClick={() => pick({ ...pickInkTool(t.tool), width: WIDTH[t.tool] * size.k })}>
            {t.icon}
          </ClusterButton>
        ))}
        <ClusterButton label="Eraser" on={mode === 'erase'} onClick={() => pick({ mode: 'erase' })}>
          <Eraser className="size-5" />
        </ClusterButton>
      </div>
      <div className="flex gap-1 px-1" role="radiogroup" aria-label="Colour">
        {(Object.keys(INK_COLOURS) as (keyof typeof INK_COLOURS)[]).map((c) => (
          <button
            key={c}
            type="button"
            role="radio"
            aria-checked={colour === c}
            aria-label={c === 'ink' ? 'Ink (black or white with the theme)' : c}
            onClick={() => pick({ colour: c })}
            className="grid size-9 place-items-center rounded-full focus-visible:outline focus-visible:outline-2 focus-visible:outline-fg-accent"
          >
            <span className="size-6 rounded-full" style={{ background: inkCss(c, dark), boxShadow: colour === c ? '0 0 0 2px var(--fg-accent)' : '0 0 0 1px color-mix(in oklab, currentColor 25%, transparent)' }} />
          </button>
        ))}
      </div>
      <div className="flex gap-1 px-1" role="radiogroup" aria-label="Width">
        {SIZES.map((s) => (
          <button
            key={s.label}
            type="button"
            role="radio"
            aria-checked={size === s}
            aria-label={s.label}
            onClick={() => pick({ width: WIDTH[tool] * s.k })}
            className="grid size-9 place-items-center rounded-full focus-visible:outline focus-visible:outline-2 focus-visible:outline-fg-accent"
            style={{ boxShadow: size === s ? 'inset 0 0 0 2px var(--fg-accent)' : undefined }}
          >
            <span className="rounded-full bg-current" style={{ width: s.dot, height: s.dot }} />
          </button>
        ))}
      </div>
    </div>
  )
}

function InkTools({ store, onActivity, onOpenChange }: { store: WbStore; onActivity(): void; onOpenChange(open: boolean): void }) {
  const tool = useStore(store, (s) => s.tool)
  const mode = useStore(store, (s) => s.mode)
  const face = toolFace(mode === 'select' || mode === 'link' || mode === 'frame' ? 'idle' : mode, tool)
  return (
    <Popover onOpenChange={onOpenChange}>
      <PopoverTrigger asChild>
        <ClusterButton label={`Ink tools: ${face.label}`} on={face.armed}>
          {face.icon}
        </ClusterButton>
      </PopoverTrigger>
      <PopoverContent side="left" align="start" className="wb-glass w-auto rounded-2xl border-0 p-2" onPointerDown={onActivity}>
        <InkPanel store={store} onActivity={onActivity} />
      </PopoverContent>
    </Popover>
  )
}

/** Add · Link · Frame · Select | Fit · Outline · Follow · Undo · Redo · ⋯ — the cluster's column and the tools menu's
 *  grid. `side`: where its Add and ⋯ popovers open. */
export function ClusterActions(p: ClusterProps & { side: 'left' | 'top'; sep: ReactNode; onHeld?(open: boolean): void }) {
  const mode = useStore(p.store, (s) => s.mode)
  const follow = useStore(p.store, (s) => s.follow)
  const [addOpen, setAddOpen] = useState(false)
  const [moreOpen, setMoreOpen] = useState(false)
  const { onHeld } = p
  useEffect(() => onHeld?.(addOpen || moreOpen), [addOpen, moreOpen, onHeld])
  // Touch: opening ⋯ must not focus its Title field (that raises the keyboard); a tap on the field focuses it.
  const coarse = usePointerCoarse()
  const toggle = (m: Mode) => {
    p.store.getState().set({ mode: mode === m ? 'idle' : m })
    p.onActivity()
  }
  return (
    <>
      <Popover open={addOpen} onOpenChange={setAddOpen}>
        <PopoverTrigger asChild>
          <ClusterButton label="Add card">
            <Plus className="size-5" />
          </ClusterButton>
        </PopoverTrigger>
        {/* No focus return to the trigger on close: the chosen kind may focus what it made (a new sticky's text). */}
        <PopoverContent side={p.side} align="start" className="wb-glass w-60 rounded-2xl border-0 p-1.5" onPointerDown={p.onActivity} onCloseAutoFocus={(e) => e.preventDefault()}>
          <ul className="flex flex-col" aria-label="Add to the whiteboard">
            {ADD.map((a) => (
              <li key={a.kind}>
                <button
                  type="button"
                  disabled={!p.addable[a.kind]}
                  onClick={() => {
                    setAddOpen(false)
                    p.onAdd(a.kind)
                  }}
                  className="wb-press flex min-h-11 w-full items-center gap-2.5 rounded-xl px-3 text-left text-sm disabled:opacity-40"
                >
                  {a.icon}
                  {a.label}
                </button>
              </li>
            ))}
          </ul>
        </PopoverContent>
      </Popover>
      <ClusterButton label="Link mode" on={mode === 'link'} onClick={() => toggle('link')}>
        <Spline className="size-5 -scale-x-100" />
      </ClusterButton>
      <ClusterButton label="Frame" on={mode === 'frame'} onClick={() => toggle('frame')}>
        <Frame className="size-5" />
      </ClusterButton>
      <ClusterButton label="Select" on={mode === 'select'} onClick={() => toggle('select')}>
        <LassoSelect className="size-5" />
      </ClusterButton>
      {p.sep}
      <ClusterButton label="Fit" onClick={p.onFit}>
        <Scan className="size-5" />
      </ClusterButton>
      <ClusterButton label="Outline view" onClick={p.onOutline}>
        <ListTree className="size-5" />
      </ClusterButton>
      <ClusterButton label="Present (Shift+P)" onClick={p.onPresent}>
        <Presentation className="size-5" />
      </ClusterButton>
      <ClusterButton
        label="Follow the agent"
        on={follow}
        onClick={() => {
          p.store.getState().set({ follow: !follow })
          p.onActivity()
        }}
      >
        <Crosshair className="size-5" />
      </ClusterButton>
      <ClusterButton label="Undo" onClick={p.onUndo} disabled={!p.canUndo}>
        <Undo2 className="size-5" />
      </ClusterButton>
      <ClusterButton label="Redo" onClick={p.onRedo} disabled={!p.canRedo}>
        <Redo2 className="size-5" />
      </ClusterButton>
      <Popover open={moreOpen} onOpenChange={setMoreOpen}>
        <PopoverTrigger asChild>
          <ClusterButton label="More">
            <Ellipsis className="size-5" />
          </ClusterButton>
        </PopoverTrigger>
        <PopoverContent
          side={p.side}
          align="end"
          className="wb-glass max-h-[70vh] w-72 overflow-y-auto rounded-2xl border-0 p-1.5"
          onPointerDown={p.onActivity}
          onOpenAutoFocus={coarse ? (e) => e.preventDefault() : undefined}
        >
          {p.more}
        </PopoverContent>
      </Popover>
    </>
  )
}

export function Cluster(p: ClusterProps) {
  const [inkOpen, setInkOpen] = useState(false)
  const [actionsHeld, setActionsHeld] = useState(false)
  const held = inkOpen || actionsHeld
  const { onHold } = p
  useEffect(() => {
    onHold(held)
    return () => onHold(false)
  }, [held, onHold])
  return (
    <div
      role="toolbar"
      aria-label="Whiteboard controls"
      aria-orientation="vertical"
      className="wb-glass wb-rise absolute right-[calc(24px+var(--safe-right,0px))] bottom-[var(--wb-dock,calc(80px+var(--safe-bottom,0px)))] z-20 flex max-h-[calc(100%-var(--wb-dock,80px)-72px-var(--safe-top,0px))] w-[58px] flex-col items-center gap-0.5 overflow-y-auto rounded-[22px] py-1.5"
      onPointerDown={p.onActivity}
      onKeyDown={p.onActivity}
    >
      <InkTools store={p.store} onActivity={p.onActivity} onOpenChange={setInkOpen} />
      <ClusterActions {...p} side="left" sep={<Sep />} onHeld={setActionsHeld} />
    </div>
  )
}

/** A menu over the canvas closes on a press on the canvas. Radix defers an outside press to its click, and React Flow's
 *  d3-zoom stops the canvas's mousedown, so Radix reads the press as handled and leaves the menu open. */
export function useCloseOnCanvas(open: boolean, close: () => void) {
  useEffect(() => {
    if (!open) return
    const press = (e: PointerEvent) => e.target instanceof Element && e.target.closest('.wb-canvas') && close()
    document.addEventListener('pointerdown', press, true)
    return () => document.removeEventListener('pointerdown', press, true)
  }, [open, close])
}

/** The composer's tools button: shows the active tool (Hand included) and opens the ink panel with the cluster's
 *  actions under it. */
export function ToolsMenu(p: Omit<ClusterProps, 'onHold'>) {
  const tool = useStore(p.store, (s) => s.tool)
  const mode = useStore(p.store, (s) => s.mode)
  const [open, setOpen] = useState(false)
  useCloseOnCanvas(open, () => setOpen(false))
  const face = toolFace(mode, tool)
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          aria-label={`Tools: ${face.label}`}
          title={`Tools: ${face.label}`}
          data-armed={face.armed}
          className="grid size-9 shrink-0 place-items-center rounded-full bg-layer-2 text-fg-primary data-[armed=true]:bg-fg-accent/15 data-[armed=true]:text-fg-accent focus-visible:outline focus-visible:outline-2 focus-visible:outline-fg-accent"
        >
          {face.icon}
        </button>
      </PopoverTrigger>
      <PopoverContent side="top" align="start" collisionPadding={12} className="wb-glass w-auto max-w-[calc(100vw-24px)] rounded-2xl border-0 p-2">
        <div className="flex flex-col gap-2">
          <InkPanel store={p.store} onActivity={p.onActivity} />
          <div className="h-px bg-current opacity-10" aria-hidden />
          <div role="toolbar" aria-label="Whiteboard controls" className="grid grid-cols-5 gap-1" onClick={(e) => e.target !== e.currentTarget && (e.target as Element).closest('[aria-haspopup]') === null && setOpen(false)}>
            <ClusterActions {...p} onHold={() => {}} side="top" sep={null} />
          </div>
        </div>
      </PopoverContent>
    </Popover>
  )
}
