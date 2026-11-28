import * as React from 'react'
import { cva, type VariantProps } from 'class-variance-authority'
import { cn } from 'cn'
import { PanelLeftIcon } from 'lucide-react'
import { Slot } from 'radix-ui'

import { useIsMobile } from '@/hooks/use-mobile'
import { touch } from '@/lib/hotkeys'
import { clamp, PX_STEP } from '@/lib/resize-drag'
import { SIDEBAR_SLIDE_EVENT } from '@/lib/sidebar-slide'
import { SWIPE_ATTR } from '@/lib/use-row-swipe'
import { ResizeHandle } from '@/components/ui/resize-handle'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Separator } from '@/components/ui/separator'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { Skeleton } from '@/components/ui/skeleton'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'

const SIDEBAR_COOKIE_NAME = 'sidebar_state'
const SIDEBAR_WIDTH_COOKIE_NAME = 'sidebar_width'
const SIDEBAR_COOKIE_MAX_AGE = 60 * 60 * 24 * 7
/** Desktop width in px: 16rem by default; the drag edge clamps it to 12rem–28rem. */
export const SIDEBAR_WIDTH = 256
export const SIDEBAR_WIDTH_MIN = 192
export const SIDEBAR_WIDTH_MAX = 448
const SIDEBAR_WIDTH_MOBILE = '18rem'
/** The collapsed desktop rail (`collapsible="icon"`): one column of 36 px icons. */
const SIDEBAR_WIDTH_ICON = '3.5rem'
const SIDEBAR_KEYBOARD_SHORTCUT = 'b'

/** The width saved in the `sidebar_width` cookie, clamped; the default when there is none. */
export function readSidebarWidth(cookie: string): number {
  const saved = /(?:^|;\s*)sidebar_width=(\d+)/.exec(cookie)
  return saved ? clamp(Number(saved[1]), SIDEBAR_WIDTH_MIN, SIDEBAR_WIDTH_MAX) : SIDEBAR_WIDTH
}

/** Whether the sidebar was expanded when last saved (`sidebar_state` cookie); expanded when there is none. */
export function readSidebarOpen(cookie: string): boolean {
  return /(?:^|;\s*)sidebar_state=(\w+)/.exec(cookie)?.[1] !== 'false'
}

/**
 * The expanded state this page last saved, ahead of the cookie. A mesh switch remounts the whole shell (App's
 * `EngineProvider key={engine}`), and the iOS/iPadOS webview keeps no cookie on the app's custom scheme, so a re-read
 * of the cookie alone fell back to expanded: a collapsed iPad sidebar sprang open on every switch.
 */
let saved: boolean | undefined

/** The expanded state a (re)mounting shell starts with: this page's last saved one, else the cookie's. */
export function initialSidebarOpen(): boolean {
  return saved ?? readSidebarOpen(globalThis.document?.cookie ?? '')
}

/** The shell's expanded state, saved on every change so a remount (a mesh switch) or the next load starts from it. For
 * a controlled `SidebarProvider`; an uncontrolled one keeps (and saves) the same state itself. */
export function useSidebarOpen(): [boolean, React.Dispatch<React.SetStateAction<boolean>>] {
  const [open, setOpen] = React.useState(initialSidebarOpen)
  React.useEffect(() => writeSidebarOpen(open), [open])
  return [open, setOpen]
}

/** Saves the expanded state for this page and in its cookie, which `readSidebarOpen` reads at the next load. */
export function writeSidebarOpen(open: boolean): void {
  saved = open
  document.cookie = `${SIDEBAR_COOKIE_NAME}=${open}; path=/; max-age=${SIDEBAR_COOKIE_MAX_AGE}`
}

type SidebarActions = {
  setOpen: (open: boolean | ((open: boolean) => boolean)) => void
  setOpenMobile: React.Dispatch<React.SetStateAction<boolean>>
  toggleSidebar: () => void
  /** Clamps to `SIDEBAR_WIDTH_MIN`–`SIDEBAR_WIDTH_MAX` and saves the width in its cookie. */
  setWidth: (px: number) => void
}

type SidebarState = {
  state: 'expanded' | 'collapsed'
  open: boolean
  openMobile: boolean
  /** Desktop width in px. */
  width: number
  /** The collapsed desktop rail is on screen: collapsed (`!open`), not a phone, and the shell has a rail. */
  rail: boolean
}

type SidebarContextProps = SidebarState & SidebarActions & { isMobile: boolean }

// Three contexts so a consumer re-renders only for what it reads: the actions never change, `isMobile` only when the
// viewport crosses the phone breakpoint, the state on every toggle or resize. `useSidebar` reads all three.
const SidebarActionsContext = React.createContext<SidebarActions | null>(null)
const SidebarMobileContext = React.createContext<boolean | null>(null)
const SidebarStateContext = React.createContext<SidebarState | null>(null)

const outside = (hook: string) => new Error(`${hook} must be used within a SidebarProvider.`)

/** Everything: re-renders on every toggle and width change. Prefer `useSidebarActions` / `useSidebarIsMobile`. */
function useSidebar(): SidebarContextProps {
  const actions = React.useContext(SidebarActionsContext)
  const isMobile = React.useContext(SidebarMobileContext)
  const state = React.useContext(SidebarStateContext)
  const all = React.useMemo(
    () => (actions && state && isMobile !== null ? { ...state, ...actions, isMobile } : null),
    [actions, state, isMobile],
  )
  if (!all) throw outside('useSidebar')
  return all
}

/** Same as `useSidebar`, returning `null` when rendered outside a `SidebarProvider` (e.g., CardSheet or standalone tests). */
function useOptionalSidebar(): SidebarContextProps | null {
  const actions = React.useContext(SidebarActionsContext)
  const isMobile = React.useContext(SidebarMobileContext)
  const state = React.useContext(SidebarStateContext)
  return React.useMemo(
    () => (actions && state && isMobile !== null ? { ...state, ...actions, isMobile } : null),
    [actions, state, isMobile],
  )
}

/** `toggleSidebar`, `setOpen`, `setOpenMobile`, `setWidth`: referentially stable, never re-renders on a toggle. */
function useSidebarActions(): SidebarActions {
  const actions = React.useContext(SidebarActionsContext)
  if (!actions) throw outside('useSidebarActions')
  return actions
}

/** Whether the sidebar is the phone Sheet; changes only when the viewport crosses the breakpoint. */
function useSidebarIsMobile(): boolean {
  const isMobile = React.useContext(SidebarMobileContext)
  if (isMobile === null) throw outside('useSidebarIsMobile')
  return isMobile
}

function SidebarProvider({
  defaultOpen,
  open: openProp,
  onOpenChange: setOpenProp,
  rail: hasRail = false,
  className,
  style,
  children,
  ...props
}: React.ComponentProps<'div'> & {
  /** Uncontrolled start; unset: this page's last saved state, else the `sidebar_state` cookie (`initialSidebarOpen`). */
  defaultOpen?: boolean
  open?: boolean
  onOpenChange?: (open: boolean) => void
  /** Collapsing leaves an icon rail on the desktop (`<Sidebar collapsible="icon">`) rather than hiding it. */
  rail?: boolean
}) {
  const isMobile = useIsMobile()
  const [openMobile, setOpenMobile] = React.useState(false)

  // Uncontrolled (the shell): this provider owns the expanded state, saves every change (`writeSidebarOpen`, so a mesh
  // switch's remount and the next load start from it) and answers ⇧⌘S's `theuth:toggle-sidebar`. Controlled (`open`):
  // the owner keeps and saves it (`useSidebarOpen`), or it is fixed (a popped window).
  const controlled = openProp !== undefined
  const [_open, _setOpen] = React.useState(() => defaultOpen ?? initialSidebarOpen())
  const open = openProp ?? _open
  const written = React.useRef(_open)
  React.useEffect(() => {
    if (controlled || written.current === _open) return
    written.current = _open
    writeSidebarOpen(_open)
  }, [controlled, _open])

  // What the stable actions below read at call time.
  const live = React.useRef({ open, controlled, setOpenProp, isMobile })
  React.useLayoutEffect(() => {
    live.current = { open, controlled, setOpenProp, isMobile }
  })
  const setOpen = React.useCallback((value: boolean | ((open: boolean) => boolean)) => {
    const l = live.current
    if (l.controlled) l.setOpenProp?.(typeof value === 'function' ? value(l.open) : value)
    else _setOpen(value)
  }, [])
  const toggleSidebar = React.useCallback(() => {
    if (live.current.isMobile) setOpenMobile((o) => !o)
    else setOpen((o) => !o)
  }, [setOpen])

  React.useEffect(() => {
    if (controlled) return
    const toggle = () => _setOpen((o) => !o)
    window.addEventListener('theuth:toggle-sidebar', toggle)
    return () => window.removeEventListener('theuth:toggle-sidebar', toggle)
  }, [controlled])

  const [width, _setWidth] = React.useState(() => readSidebarWidth(globalThis.document?.cookie ?? ''))
  const setWidth = React.useCallback((px: number) => {
    const w = clamp(Math.round(px), SIDEBAR_WIDTH_MIN, SIDEBAR_WIDTH_MAX)
    _setWidth(w)
    document.cookie = `${SIDEBAR_WIDTH_COOKIE_NAME}=${w}; path=/; max-age=${SIDEBAR_COOKIE_MAX_AGE}`
  }, [])

  // Adds a keyboard shortcut to toggle the sidebar.
  React.useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === SIDEBAR_KEYBOARD_SHORTCUT && (event.metaKey || event.ctrlKey)) {
        event.preventDefault()
        toggleSidebar()
      }
    }

    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [toggleSidebar])

  // We add a state so that we can do data-state="expanded" or "collapsed".
  // This makes it easier to style the sidebar with Tailwind classes.
  const state = open ? 'expanded' : 'collapsed'
  const rail = hasRail && !isMobile && !open

  const actions = React.useMemo<SidebarActions>(() => ({ setOpen, setOpenMobile, toggleSidebar, setWidth }), [setOpen, toggleSidebar, setWidth])
  const stateValue = React.useMemo<SidebarState>(() => ({ state, open, openMobile, width, rail }), [state, open, openMobile, width, rail])

  return (
    <SidebarActionsContext.Provider value={actions}>
      <SidebarMobileContext.Provider value={isMobile}>
        <SidebarStateContext.Provider value={stateValue}>
          <TooltipProvider delayDuration={0}>
            <div
              data-slot="sidebar-wrapper"
              style={
                {
                  '--sidebar-width': `${width}px`,
                  '--sidebar-width-icon': SIDEBAR_WIDTH_ICON,
                  ...style,
                } as React.CSSProperties
              }
              // `overflow-x-clip`: the column slides right while the sidebar expands (`Sidebar`); its far edge is cut, never scrolled to.
              className={cn(
                'group/sidebar-wrapper flex min-h-svh w-full overflow-x-clip has-data-[variant=inset]:bg-sidebar',
                className,
              )}
              {...props}
            >
              {children}
            </div>
          </TooltipProvider>
        </SidebarStateContext.Provider>
      </SidebarMobileContext.Provider>
    </SidebarActionsContext.Provider>
  )
}

/** The desktop open/close slide: compositor-only `transform`, never an animated width. */
const SLIDE_MS = 200
const SLIDE_EASE = 'cubic-bezier(0.22, 1, 0.36, 1)'

/** `el`'s current horizontal translation (px), mid-animation included. */
function translateXOf(el: Element): number {
  const m = /^matrix(3d)?\((.+)\)$/.exec(getComputedStyle(el).transform)
  if (!m) return 0
  const v = m[2]!.split(',').map(Number)
  return (m[1] ? v[12] : v[4]) ?? 0
}

function Sidebar({
  side = 'left',
  variant = 'sidebar',
  collapsible = 'offcanvas',
  className,
  style,
  children,
  ...props
}: React.ComponentProps<'div'> & {
  side?: 'left' | 'right'
  variant?: 'sidebar' | 'floating' | 'inset'
  collapsible?: 'offcanvas' | 'icon' | 'none'
}) {
  const { isMobile, state, open, setOpen, openMobile, setOpenMobile, width, setWidth } = useSidebar()
  const [resizing, setResizing] = React.useState(false)

  // Touch (phones and tablets): a rightward swipe from the screen's left edge opens the sidebar; a leftward swipe
  // that starts inside it (its own element, whatever its width) closes it. Gestures elsewhere are left to the content
  // (an Inbox card's swipe, scrolling), and so is a swipe that starts on a row with its own swipe (`SWIPE_ATTR`).
  React.useEffect(() => {
    if (!isMobile && !touch()) return
    let start: { x: number; y: number; axis: 'x' | 'y' | null; inside: boolean } | null = null
    const down = (e: globalThis.PointerEvent) => {
      if (e.pointerType !== 'touch' || !e.isPrimary) return
      if (!openMobile && !open && e.clientX > 24) return
      const target = e.target instanceof Element ? e.target : null
      const inside = !!target?.closest('[data-sidebar="sidebar"]') && !target?.closest(`[${SWIPE_ATTR}]`)
      start = { x: e.clientX, y: e.clientY, axis: null, inside }
    }
    const move = (e: globalThis.PointerEvent) => {
      if (!start) return
      const dx = e.clientX - start.x
      const dy = e.clientY - start.y
      if (start.axis === null && (Math.abs(dx) > 12 || Math.abs(dy) > 12)) start.axis = Math.abs(dx) > Math.abs(dy) ? 'x' : 'y'
      if (start.axis === 'y') start = null
    }
    const up = (e: globalThis.PointerEvent) => {
      if (!start) return
      const s = start
      start = null
      if (s.axis !== 'x') return
      const dx = e.clientX - s.x
      if (dx > 64 && s.x <= 24) {
        if (isMobile) setOpenMobile(true)
        else setOpen(true)
      } else if (dx < -64 && s.inside) {
        if (isMobile) setOpenMobile(false)
        else setOpen(false)
      }
    }
    document.addEventListener('pointerdown', down, { passive: true })
    document.addEventListener('pointermove', move, { passive: true })
    document.addEventListener('pointerup', up, { passive: true })
    document.addEventListener('pointercancel', up, { passive: true })
    return () => {
      document.removeEventListener('pointerdown', down)
      document.removeEventListener('pointermove', move)
      document.removeEventListener('pointerup', up)
      document.removeEventListener('pointercancel', up)
    }
  }, [isMobile, open, openMobile, setOpen, setOpenMobile])
  const start = React.useRef({ x: 0, width: 0 })
  // A left sidebar grows as its edge moves right; a right one as it moves left.
  const dir = side === 'left' ? 1 : -1

  // Desktop open/close. The container is always `--sidebar-width` wide (its content never reflows); collapsed it is
  // translated so only its trailing `railGap` shows (the icon rail's content sits at that edge; offcanvas: none of it).
  // Only the gap's width moves the column, once per toggle: collapsing commits the narrow gap at once and slides the
  // (now wider) column in from where it was; expanding keeps the narrow gap while the column slides right (its far edge
  // clipped by the wrapper), then commits the wide gap and drops the slide in the same frame. Both slides are Web
  // Animations of `transform` alone; a toggle mid-slide picks up from where things are.
  const railGap =
    collapsible === 'icon' ? (variant === 'sidebar' ? 'var(--sidebar-width-icon)' : 'calc(var(--sidebar-width-icon) + 1rem)') : '0px'
  const root = React.useRef<HTMLDivElement>(null)
  const gap = React.useRef<HTMLDivElement>(null)
  const box = React.useRef<HTMLDivElement>(null)
  const slide = React.useRef<Animation[] | null>(null)
  const shown = React.useRef(state)
  React.useLayoutEffect(() => {
    const was = shown.current
    shown.current = state
    if (was === state) return
    const g = gap.current
    const b = box.current
    const prev = slide.current
    slide.current = null
    const inset = root.current?.parentElement?.querySelector<HTMLElement>(':scope > [data-slot="sidebar-inset"]') ?? null
    // Read where a running slide has things before dropping it.
    const boxAt = prev && b ? translateXOf(b) : null
    const insetAt = prev && inset ? translateXOf(inset) : null
    for (const a of prev ?? []) a.cancel()
    if (!g || !b) return
    g.style.width = ''
    if (typeof b.animate !== 'function' || globalThis.matchMedia?.('(prefers-reduced-motion: reduce)').matches) return
    const expanding = state === 'expanded'
    const rem = parseFloat(getComputedStyle(document.documentElement).fontSize) || 16
    const railPx = collapsible === 'icon' ? rem * (variant === 'sidebar' ? 3.5 : 4.5) : 0
    const delta = (width - railPx) * dir
    const boxFrom = boxAt ?? (expanding ? -delta : 0)
    const boxTo = expanding ? 0 : -delta
    const duration = delta === 0 ? 0 : SLIDE_MS * Math.min(1, Math.abs(boxTo - boxFrom) / Math.abs(delta))
    if (duration < 1) return
    const timing: KeyframeAnimationOptions = { duration, easing: SLIDE_EASE, fill: 'forwards' }
    const at = (x: number) => ({ transform: `translateX(${x}px)` })
    if (expanding) g.style.width = railGap
    const anims = [b.animate([at(boxFrom), at(boxTo)], timing)]
    if (inset) anims.push(inset.animate([at(insetAt ?? (expanding ? 0 : delta)), at(expanding ? delta : 0)], timing))
    slide.current = anims
    window.dispatchEvent(new Event(SIDEBAR_SLIDE_EVENT))
    anims[0]!.finished.then(
      () => {
        if (slide.current !== anims) return
        slide.current = null
        // One task, so one frame: the final gap and the column's rest position land together (one reflow).
        g.style.width = ''
        for (const a of anims) a.cancel()
      },
      () => undefined,
    )
  }, [state, width, collapsible, variant, dir, railGap])
  React.useEffect(() => () => slide.current?.forEach((a) => a.cancel()), [])

  if (collapsible === 'none') {
    return (
      <div
        data-slot="sidebar"
        className={cn(
          'flex h-full w-(--sidebar-width) flex-col bg-sidebar text-sidebar-foreground',
          className,
        )}
        style={style}
        {...props}
      >
        {children}
      </div>
    )
  }

  if (isMobile) {
    return (
      <Sheet open={openMobile} onOpenChange={setOpenMobile} {...props}>
        <SheetContent
          data-sidebar="sidebar"
          data-slot="sidebar"
          data-mobile="true"
          className="w-(--sidebar-width) bg-sidebar p-0 pt-[var(--safe-top)] pb-[var(--safe-bottom)] pl-[var(--safe-left)] text-sidebar-foreground [&>button]:hidden"
          style={
            {
              '--sidebar-width': SIDEBAR_WIDTH_MOBILE,
            } as React.CSSProperties
          }
          side={side}
        >
          <SheetHeader className="sr-only">
            <SheetTitle>Sidebar</SheetTitle>
            <SheetDescription>Displays the mobile sidebar.</SheetDescription>
          </SheetHeader>
          <div className="flex h-full w-full flex-col">{children}</div>
        </SheetContent>
      </Sheet>
    )
  }

  const collapsed = state === 'collapsed'
  return (
    <div
      ref={root}
      className="group peer hidden text-sidebar-foreground md:block"
      data-state={state}
      data-resizing={resizing || undefined}
      data-collapsible={collapsed ? collapsible : ''}
      data-variant={variant}
      data-side={side}
      data-slot="sidebar"
    >
      {/* This is what handles the sidebar gap on desktop: the only box whose width moves the column. */}
      <div
        ref={gap}
        data-slot="sidebar-gap"
        className={cn(
          'relative w-(--sidebar-width) bg-transparent',
          'group-data-[collapsible=offcanvas]:w-0',
          'group-data-[side=right]:rotate-180',
          variant === 'floating' || variant === 'inset'
            ? 'group-data-[collapsible=icon]:w-[calc(var(--sidebar-width-icon)+(--spacing(4)))]'
            : 'group-data-[collapsible=icon]:w-(--sidebar-width-icon)',
        )}
      />
      {/* The shell's own height (`h-band`: `--visible-band`, lib/viewport.ts), never a CSS viewport unit: the iOS/iPadOS
       * webview can leave `100svh` stale and short for the life of the page, which squeezed the rail's scrolling middle
       * (between its fixed header and Settings) to nothing until the app restarted. 217: less the status bar under it
       * (`--statusbar-h`, set by the shell; unset where there is no bar). Always `--sidebar-width` wide; collapsed, it is
       * translated so only its trailing `railGap` shows. */}
      <div
        ref={box}
        data-slot="sidebar-container"
        className={cn(
          'fixed top-0 z-10 hidden h-band max-h-[calc(var(--visible-band)_-_var(--statusbar-h,0px))] w-(--sidebar-width) md:flex',
          side === 'left' ? 'left-0' : 'right-0',
          // Adjust the padding for floating and inset variants.
          variant === 'floating' || variant === 'inset'
            ? 'p-2'
            : 'group-data-[side=left]:border-r group-data-[side=right]:border-l',
          className,
        )}
        style={
          collapsed
            ? { transform: `translateX(calc((${railGap} - var(--sidebar-width)) * ${dir}))`, ...style }
            : style
        }
        {...props}
      >
        <div
          data-sidebar="sidebar"
          data-slot="sidebar-inner"
          className="relative flex h-full w-full flex-col bg-sidebar pt-[var(--safe-top)] pb-[var(--safe-bottom)] pl-[var(--safe-left)] group-data-[variant=floating]:rounded-lg group-data-[variant=floating]:border group-data-[variant=floating]:border-sidebar-border group-data-[variant=floating]:shadow-sm md:pb-0"
        >
          {children}
        </div>
        {/* Expanded desktop only: collapsed (offcanvas or icon) and the phone Sheet have no drag edge. The hit zone
         * reaches away from the sidebar so its scrollbar stays usable. A drag resizes live: no slide runs. */}
        {state === 'expanded' && (
          <ResizeHandle
            orientation="vertical"
            placement={side === 'left' ? 'end' : 'start'}
            aria-label="Resize sidebar"
            className={cn('absolute inset-y-0 bg-transparent', side === 'left' ? 'right-0' : 'left-0')}
            value={{ now: width, min: SIDEBAR_WIDTH_MIN, max: SIDEBAR_WIDTH_MAX }}
            onStart={(e) => {
              start.current = { x: e.clientX, width }
              setResizing(true)
            }}
            onMove={(x) => setWidth(start.current.width + dir * (x - start.current.x))}
            onEnd={() => setResizing(false)}
            onNudge={(d) => setWidth(width + dir * d * PX_STEP)}
            onReset={() => setWidth(SIDEBAR_WIDTH)}
          />
        )}
      </div>
    </div>
  )
}

function SidebarTrigger({ className, onClick, ...props }: React.ComponentProps<typeof Button>) {
  const { toggleSidebar } = useSidebarActions()

  return (
    <Button
      data-sidebar="trigger"
      data-slot="sidebar-trigger"
      variant="ghost"
      size="icon"
      className={cn('size-7', className)}
      onClick={(event) => {
        onClick?.(event)
        toggleSidebar()
      }}
      {...props}
    >
      <PanelLeftIcon />
      <span className="sr-only">Toggle Sidebar</span>
    </Button>
  )
}

function SidebarRail({ className, ...props }: React.ComponentProps<'button'>) {
  const { toggleSidebar } = useSidebarActions()

  return (
    <button
      data-sidebar="rail"
      data-slot="sidebar-rail"
      aria-label="Toggle Sidebar"
      tabIndex={-1}
      onClick={toggleSidebar}
      title="Toggle Sidebar"
      className={cn(
        'absolute inset-y-0 z-20 hidden w-4 -translate-x-1/2 transition-all ease-linear group-data-[side=left]:-right-4 group-data-[side=right]:left-0 after:absolute after:inset-y-0 after:left-1/2 after:w-[2px] hover:after:bg-sidebar-border sm:flex',
        'in-data-[side=left]:cursor-w-resize in-data-[side=right]:cursor-e-resize',
        '[[data-side=left][data-state=collapsed]_&]:cursor-e-resize [[data-side=right][data-state=collapsed]_&]:cursor-w-resize',
        'group-data-[collapsible=offcanvas]:translate-x-0 group-data-[collapsible=offcanvas]:after:left-full hover:group-data-[collapsible=offcanvas]:bg-sidebar',
        '[[data-side=left][data-collapsible=offcanvas]_&]:-right-2',
        '[[data-side=right][data-collapsible=offcanvas]_&]:-left-2',
        className,
      )}
      {...props}
    />
  )
}

function SidebarInset({ className, ...props }: React.ComponentProps<'main'>) {
  return (
    <main
      data-slot="sidebar-inset"
      className={cn(
        'relative flex w-full flex-1 flex-col bg-background',
        'md:peer-data-[variant=inset]:m-2 md:peer-data-[variant=inset]:ml-0 md:peer-data-[variant=inset]:rounded-xl md:peer-data-[variant=inset]:shadow-sm md:peer-data-[variant=inset]:peer-data-[state=collapsed]:ml-2',
        className,
      )}
      {...props}
    />
  )
}

function SidebarInput({ className, ...props }: React.ComponentProps<typeof Input>) {
  return (
    <Input
      data-slot="sidebar-input"
      data-sidebar="input"
      className={cn('h-8 w-full bg-background shadow-none', className)}
      {...props}
    />
  )
}

function SidebarHeader({ className, ...props }: React.ComponentProps<'div'>) {
  return (
    <div
      data-slot="sidebar-header"
      data-sidebar="header"
      className={cn('flex flex-col gap-2 p-2', className)}
      {...props}
    />
  )
}

function SidebarFooter({ className, ...props }: React.ComponentProps<'div'>) {
  return (
    <div
      data-slot="sidebar-footer"
      data-sidebar="footer"
      className={cn('flex flex-col gap-2 p-2', className)}
      {...props}
    />
  )
}

function SidebarSeparator({ className, ...props }: React.ComponentProps<typeof Separator>) {
  return (
    <Separator
      data-slot="sidebar-separator"
      data-sidebar="separator"
      className={cn('mx-2 w-auto bg-sidebar-border', className)}
      {...props}
    />
  )
}

function SidebarContent({ className, ...props }: React.ComponentProps<'div'>) {
  return (
    <div
      data-slot="sidebar-content"
      data-sidebar="content"
      className={cn(
        'flex min-h-0 flex-1 flex-col gap-2 overflow-auto',
        className,
      )}
      {...props}
    />
  )
}

function SidebarGroup({ className, ...props }: React.ComponentProps<'div'>) {
  return (
    <div
      data-slot="sidebar-group"
      data-sidebar="group"
      className={cn('relative flex w-full min-w-0 flex-col p-2', className)}
      {...props}
    />
  )
}

function SidebarGroupLabel({
  className,
  asChild = false,
  ...props
}: React.ComponentProps<'div'> & { asChild?: boolean }) {
  const Comp = asChild ? Slot.Root : 'div'

  return (
    <Comp
      data-slot="sidebar-group-label"
      data-sidebar="group-label"
      className={cn(
        'flex h-8 shrink-0 items-center rounded-md px-2 text-xs font-medium text-sidebar-foreground/70 ring-sidebar-ring outline-hidden focus-visible:ring-2 [&>svg]:size-4 [&>svg]:shrink-0',
        className,
      )}
      {...props}
    />
  )
}

function SidebarGroupAction({
  className,
  asChild = false,
  ...props
}: React.ComponentProps<'button'> & { asChild?: boolean }) {
  const Comp = asChild ? Slot.Root : 'button'

  return (
    <Comp
      data-slot="sidebar-group-action"
      data-sidebar="group-action"
      className={cn(
        'absolute top-3.5 right-3 flex aspect-square w-5 items-center justify-center rounded-md p-0 text-sidebar-foreground ring-sidebar-ring outline-hidden transition-transform hover:bg-sidebar-accent hover:text-sidebar-accent-foreground focus-visible:ring-2 [&>svg]:size-4 [&>svg]:shrink-0',
        // Increases the hit area of the button on mobile.
        'after:absolute after:-inset-2 md:after:hidden',
        className,
      )}
      {...props}
    />
  )
}

function SidebarGroupContent({ className, ...props }: React.ComponentProps<'div'>) {
  return (
    <div
      data-slot="sidebar-group-content"
      data-sidebar="group-content"
      className={cn('w-full text-sm', className)}
      {...props}
    />
  )
}

function SidebarMenu({ className, ...props }: React.ComponentProps<'ul'>) {
  return (
    <ul
      data-slot="sidebar-menu"
      data-sidebar="menu"
      className={cn('flex w-full min-w-0 flex-col gap-1', className)}
      {...props}
    />
  )
}

function SidebarMenuItem({ className, ...props }: React.ComponentProps<'li'>) {
  return (
    <li
      data-slot="sidebar-menu-item"
      data-sidebar="menu-item"
      className={cn('group/menu-item relative', className)}
      {...props}
    />
  )
}

const sidebarMenuButtonVariants = cva(
  'peer/menu-button flex w-full items-center gap-2 overflow-hidden rounded-md p-2 text-left text-sm ring-sidebar-ring outline-hidden group-has-data-[sidebar=menu-action]/menu-item:pr-8 hover:bg-sidebar-accent hover:text-sidebar-accent-foreground focus-visible:ring-2 active:bg-sidebar-accent active:text-sidebar-accent-foreground disabled:pointer-events-none disabled:opacity-50 aria-disabled:pointer-events-none aria-disabled:opacity-50 data-[active=true]:bg-sidebar-accent data-[active=true]:font-medium data-[active=true]:text-sidebar-accent-foreground data-[state=open]:hover:bg-sidebar-accent data-[state=open]:hover:text-sidebar-accent-foreground [&>span:last-child]:truncate [&>svg]:size-4 [&>svg]:shrink-0',
  {
    variants: {
      variant: {
        default: 'hover:bg-sidebar-accent hover:text-sidebar-accent-foreground',
        outline:
          'bg-background shadow-[0_0_0_1px_var(--sidebar-border)] hover:bg-sidebar-accent hover:text-sidebar-accent-foreground hover:shadow-[0_0_0_1px_var(--sidebar-accent)]',
      },
      size: {
        default: 'h-8 text-sm pointer-coarse:h-11',
        sm: 'h-7 text-xs',
        lg: 'h-12 text-sm',
      },
    },
    defaultVariants: {
      variant: 'default',
      size: 'default',
    },
  },
)

function SidebarMenuButton({
  asChild = false,
  isActive = false,
  variant = 'default',
  size = 'default',
  tooltip,
  className,
  ...props
}: React.ComponentProps<'button'> & {
  asChild?: boolean
  isActive?: boolean
  tooltip?: string | React.ComponentProps<typeof TooltipContent>
} & VariantProps<typeof sidebarMenuButtonVariants>) {
  const Comp = asChild ? Slot.Root : 'button'

  const button = (
    <Comp
      data-slot="sidebar-menu-button"
      data-sidebar="menu-button"
      data-size={size}
      data-active={isActive}
      className={cn(sidebarMenuButtonVariants({ variant, size }), className)}
      {...props}
    />
  )

  if (!tooltip) {
    return button
  }

  if (typeof tooltip === 'string') {
    tooltip = {
      children: tooltip,
    }
  }

  return (
    <Tooltip>
      <TooltipTrigger asChild>{button}</TooltipTrigger>
      <SidebarMenuTooltip {...tooltip} />
    </Tooltip>
  )
}

/** A menu button's tooltip, shown only on the collapsed desktop sidebar: the one part of a menu button that reads the
 * sidebar's state, so a toggle does not re-render every row. */
function SidebarMenuTooltip(props: React.ComponentProps<typeof TooltipContent>) {
  const { isMobile, state } = useSidebar()
  return <TooltipContent side="right" align="center" hidden={state !== 'collapsed' || isMobile} {...props} />
}

function SidebarMenuAction({
  className,
  asChild = false,
  showOnHover = false,
  ...props
}: React.ComponentProps<'button'> & {
  asChild?: boolean
  showOnHover?: boolean
}) {
  const Comp = asChild ? Slot.Root : 'button'

  return (
    <Comp
      data-slot="sidebar-menu-action"
      data-sidebar="menu-action"
      className={cn(
        'absolute top-1.5 right-1 flex aspect-square w-5 items-center justify-center rounded-md p-0 text-sidebar-foreground ring-sidebar-ring outline-hidden transition-transform peer-hover/menu-button:text-sidebar-accent-foreground hover:bg-sidebar-accent hover:text-sidebar-accent-foreground focus-visible:ring-2 [&>svg]:size-4 [&>svg]:shrink-0',
        // Increases the hit area of the button where the pointer is a finger (phones, iPads); a mouse never needs it.
        'after:absolute after:-inset-2 md:pointer-fine:after:hidden',
        'peer-data-[size=sm]/menu-button:top-1',
        'peer-data-[size=default]/menu-button:top-1.5',
        'peer-data-[size=lg]/menu-button:top-2.5',
        // Hidden until hover / focus / open only where a hover-capable fine pointer exists: an iPad (≥ md, coarse pointer) always shows it.
        showOnHover &&
          'group-focus-within/menu-item:opacity-100 group-hover/menu-item:opacity-100 peer-data-[active=true]/menu-button:text-sidebar-accent-foreground data-[state=open]:opacity-100 md:pointer-fine:opacity-0',
        className,
      )}
      {...props}
    />
  )
}

function SidebarMenuBadge({ className, ...props }: React.ComponentProps<'div'>) {
  return (
    <div
      data-slot="sidebar-menu-badge"
      data-sidebar="menu-badge"
      className={cn(
        'pointer-events-none absolute right-1 flex h-5 min-w-5 items-center justify-center rounded-md px-1 text-xs font-medium text-sidebar-foreground tabular-nums select-none',
        'peer-hover/menu-button:text-sidebar-accent-foreground peer-data-[active=true]/menu-button:text-sidebar-accent-foreground',
        'peer-data-[size=sm]/menu-button:top-1',
        'peer-data-[size=default]/menu-button:top-1.5',
        'peer-data-[size=lg]/menu-button:top-2.5',
        className,
      )}
      {...props}
    />
  )
}

function SidebarMenuSkeleton({
  className,
  showIcon = false,
  ...props
}: React.ComponentProps<'div'> & {
  showIcon?: boolean
}) {
  // Random width between 50 to 90%.
  const width = React.useMemo(() => {
    return `${Math.floor(Math.random() * 40) + 50}%`
  }, [])

  return (
    <div
      data-slot="sidebar-menu-skeleton"
      data-sidebar="menu-skeleton"
      className={cn('flex h-8 items-center gap-2 rounded-md px-2', className)}
      {...props}
    >
      {showIcon && <Skeleton className="size-4 rounded-md" data-sidebar="menu-skeleton-icon" />}
      <Skeleton
        className="h-4 max-w-(--skeleton-width) flex-1"
        data-sidebar="menu-skeleton-text"
        style={
          {
            '--skeleton-width': width,
          } as React.CSSProperties
        }
      />
    </div>
  )
}

function SidebarMenuSub({ className, ...props }: React.ComponentProps<'ul'>) {
  return (
    <ul
      data-slot="sidebar-menu-sub"
      data-sidebar="menu-sub"
      className={cn(
        'mx-3.5 flex min-w-0 translate-x-px flex-col gap-1 border-l border-sidebar-border px-2.5 py-0.5',
        className,
      )}
      {...props}
    />
  )
}

function SidebarMenuSubItem({ className, ...props }: React.ComponentProps<'li'>) {
  return (
    <li
      data-slot="sidebar-menu-sub-item"
      data-sidebar="menu-sub-item"
      className={cn('group/menu-sub-item relative', className)}
      {...props}
    />
  )
}

function SidebarMenuSubButton({
  asChild = false,
  size = 'md',
  isActive = false,
  className,
  ...props
}: React.ComponentProps<'a'> & {
  asChild?: boolean
  size?: 'sm' | 'md'
  isActive?: boolean
}) {
  const Comp = asChild ? Slot.Root : 'a'

  return (
    <Comp
      data-slot="sidebar-menu-sub-button"
      data-sidebar="menu-sub-button"
      data-size={size}
      data-active={isActive}
      className={cn(
        'flex h-7 min-w-0 -translate-x-px items-center gap-2 overflow-hidden rounded-md px-2 text-sidebar-foreground ring-sidebar-ring outline-hidden hover:bg-sidebar-accent hover:text-sidebar-accent-foreground focus-visible:ring-2 active:bg-sidebar-accent active:text-sidebar-accent-foreground disabled:pointer-events-none disabled:opacity-50 aria-disabled:pointer-events-none aria-disabled:opacity-50 [&>span:last-child]:truncate [&>svg]:size-4 [&>svg]:shrink-0 [&>svg]:text-sidebar-accent-foreground',
        'data-[active=true]:bg-sidebar-accent data-[active=true]:text-sidebar-accent-foreground',
        size === 'sm' && 'text-xs',
        size === 'md' && 'text-sm',
        className,
      )}
      {...props}
    />
  )
}

export {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupAction,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarInput,
  SidebarInset,
  SidebarMenu,
  SidebarMenuAction,
  SidebarMenuBadge,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarMenuSkeleton,
  SidebarMenuSub,
  SidebarMenuSubButton,
  SidebarMenuSubItem,
  SidebarProvider,
  SidebarRail,
  SidebarSeparator,
  SidebarTrigger,
  useOptionalSidebar,
  useSidebar,
  useSidebarActions,
  useSidebarIsMobile,
}
