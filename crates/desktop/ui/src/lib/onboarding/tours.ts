// 215 §Main tour: the spotlight tours as data (anchors are `[data-tour="…"]` in the real UI) and `startTour`, which
// moves to the tour's page, waits for its anchors, drops the ones that never appear (capability-hidden rows, a hidden
// checklist) and drives driver.js. The steps are resolved before the first popover, so "2 of 7" counts only shown steps.

import type { UseNavigateResult } from '@tanstack/react-router'
import { type Alignment, type Driver, type Side, driver } from 'driver.js'
import { settings } from '@/components/chat/slash'
import { groupOf } from '@/lib/nav'
import { isPopout } from '@/lib/popout'
import { isIsland, isOverlay } from '@/lib/tauri'
import type { TourId } from './state'

export type TourStep = { anchor: string; title: string; body: string; side?: Side; align?: Alignment }
/** Where a tour starts: a page to open first, or `null` to run where the user is. */
export type TourRoute = 'attention' | 'library' | 'costs' | 'devices' | 'chat' | null
export type Tour = { route: TourRoute; steps: readonly TourStep[] }

const COMPOSER: TourStep = {
  anchor: 'chat-composer',
  title: 'Tell your agent what to do',
  body: 'Ask in plain words; it runs commands, edits files and browses on this Mac. Keep typing while it works: a message sent mid-turn steers the run without stopping it. Drop files in, @ to reference one, / for commands.',
  side: 'top',
}

export const TOURS: Readonly<Record<TourId, Tour>> = {
  main: {
    route: null,
    steps: [
      COMPOSER,
      {
        anchor: 'nav-inbox',
        title: 'Inbox: what needs you',
        body: 'Approvals for commands and edits, questions agents ask, decisions they escalate and finished work land here. The badge counts what is waiting, so you can leave agents running and come back.',
        side: 'right',
      },
      {
        anchor: 'nav-library',
        title: 'Library: everything agents make',
        body: 'Pages, files and images agents produce are kept as artifacts, beside Knowledge, Skills, Notes and Notebooks. All of it is searchable.',
        side: 'right',
      },
      {
        anchor: 'nav-costs',
        title: 'Costs: spend and limits',
        body: 'Spend by model, provider and session, your plan’s usage windows, and what your subscription usage would have cost at API rates.',
        side: 'right',
      },
      {
        anchor: 'nav-agents',
        title: 'Agents: work that runs without you',
        body: 'Standing agents with a desk of their own, tasks on a board and routines on a schedule.',
        side: 'right',
      },
      {
        anchor: 'nav-settings',
        title: 'Settings',
        body: 'Model providers and defaults, your devices, Slack, Telegram and Signal, approvals and appearance.',
        side: 'right',
        align: 'end',
      },
      {
        anchor: 'onboarding-checklist',
        title: 'Keep going here',
        body: 'This list ticks itself off as you use theuth. Show me on any item runs a short tour of it.',
        side: 'right',
        align: 'end',
      },
    ],
  },
  inbox: {
    route: 'attention',
    steps: [
      {
        anchor: 'inbox-list',
        title: 'Your Inbox',
        body: 'Each card is something an agent is waiting on. Allow or deny an approval on the card itself, or open it for the full context. Keys: J and K move, A allows, D denies, S snoozes, E dismisses.',
        side: 'right',
        align: 'start',
      },
    ],
  },
  library: {
    route: 'library',
    steps: [
      {
        anchor: 'library-tabs',
        title: 'The Library',
        body: 'One tab per kind of thing agents keep for you: artifacts they made, knowledge they learned, skills they can use, your notes and notebooks.',
        side: 'bottom',
      },
      {
        anchor: 'library-overview',
        title: 'At a glance',
        body: 'What each tab holds and how much is in it. Pick a tile to open the tab.',
        side: 'bottom',
      },
    ],
  },
  costs: {
    route: 'costs',
    steps: [
      {
        anchor: 'costs-subscription-value',
        title: 'Subscription value',
        body: 'Calls through a Claude or ChatGPT plan have no per-token bill. This is what they would have cost at API rates, this month and over 30 days.',
        side: 'bottom',
      },
      {
        anchor: 'costs-limits',
        title: 'Usage limits',
        body: 'Your plan’s 5-hour and weekly windows with the pace you are on, rate limits for API keys, and any budgets you set.',
        side: 'top',
      },
    ],
  },
  devices: {
    route: 'devices',
    steps: [
      {
        anchor: 'devices-pair',
        title: 'Link your phone',
        body: 'Pair a device shows a QR code and a key, valid for five minutes. Paste the key into the theuth app on your phone, and it reaches this Mac’s agents from anywhere.',
        side: 'bottom',
        align: 'end',
      },
    ],
  },
  chat: {
    route: 'chat',
    steps: [
      COMPOSER,
      {
        anchor: 'chat-model',
        title: 'Pick the model',
        body: 'Switch model and effort for this chat at any time; /model opens the same picker.',
        side: 'bottom',
        align: 'start',
      },
      {
        anchor: 'chat-context',
        title: 'Context used',
        body: 'How full the model’s context window is. Long chats are compacted for you before they run out.',
        side: 'bottom',
      },
    ],
  },
}

export const tourSelector = (anchor: string): string => `[data-tour="${anchor}"]`

/** Wait for every anchor this long; then run with the ones present. */
export const TOUR_WAIT_MS = 4_000
/** No anchor at all yet (a page still opening, the chat view still mounting): wait up to this, then give up. */
export const TOUR_GIVE_UP_MS = 10_000

/** Whether the wait for anchors is over: all present, or the wait ran out with something to show, or nothing came at all. */
export function settled(elapsed: number, present: number, total: number): boolean {
  return present === total || (elapsed >= TOUR_WAIT_MS && present > 0) || elapsed >= TOUR_GIVE_UP_MS
}

/** The steps to drive: not skipped, and whose anchor is on screen. */
export function presentSteps(steps: readonly TourStep[], has: (anchor: string) => boolean, skip: readonly string[] = []): TourStep[] {
  return steps.filter((s) => !skip.includes(s.anchor) && has(s.anchor))
}

/** An anchor on screen: in the DOM and laid out (a `display: none` copy, the rail's twin on a phone, does not count). */
const visible = (anchor: string): boolean => {
  const el = document.querySelector(tourSelector(anchor))
  return !!el && el.getClientRects().length > 0
}

type Navigate = UseNavigateResult<string>

function go(route: TourRoute, navigate: Navigate): void {
  const path = location.pathname
  if (route === 'attention' && !path.startsWith('/attention')) void navigate({ to: '/attention' })
  else if (route === 'library' && groupOf(path) !== 'library') void navigate({ to: '/artifacts' })
  else if (route === 'costs' && path !== '/costs') void navigate({ to: '/costs' })
  else if (route === 'devices') void settings(navigate, 'devices')
  else if (route === 'chat' && !visible(COMPOSER.anchor)) void navigate({ to: '/new' })
}

let active: Driver | null = null
let run = 0

/** Stop any running tour (a new one replaces it). */
export function stopTour(): void {
  run++
  active?.destroy()
  active = null
}

/**
 * Open the tour's page, wait for its anchors (`settled`), then spotlight the ones present. Esc, the overlay and
 * Skip tour end it. Never in a pop-out, the island or an overlay window. `skip`: anchors known to be absent.
 */
export function startTour(id: TourId, navigate: Navigate, opts: { skip?: readonly string[] } = {}): void {
  if (isPopout() || isIsland() || isOverlay()) return
  stopTour()
  const token = run
  const tour = TOURS[id]
  const wanted = tour.steps.filter((s) => !opts.skip?.includes(s.anchor))
  if (!wanted.length) return
  go(tour.route, navigate)
  const start = Date.now()
  const tick = (): void => {
    if (token !== run) return
    const steps = presentSteps(wanted, visible)
    if (!settled(Date.now() - start, steps.length, wanted.length)) return void setTimeout(tick, 150)
    if (!steps.length) return
    const d = driver({
      steps: steps.map((s) => ({
        element: tourSelector(s.anchor),
        // Gone between the wait and its turn (a page change): passed over rather than shown unanchored.
        skipMissingElement: true,
        popover: { title: s.title, description: s.body, side: s.side, align: s.align },
      })),
      popoverClass: 'theuth-tour',
      showProgress: steps.length > 1,
      progressText: '{{current}} of {{total}}',
      showButtons: ['next', 'previous'],
      nextBtnText: 'Next',
      prevBtnText: 'Back',
      doneBtnText: 'Done',
      allowClose: true,
      allowKeyboardControl: true,
      overlayClickBehavior: 'close',
      // The tint is a token (tour.css), so it follows light and dark; the alpha is in the colour.
      overlayColor: 'var(--tour-overlay)',
      overlayOpacity: 1,
      stagePadding: 6,
      stageRadius: 12,
      popoverOffset: 12,
      smoothScroll: true,
      focus: 'popover',
      onPopoverRender: (popover, { driver: dr }) => {
        if (dr.isLastStep()) return
        const skip = document.createElement('button')
        skip.type = 'button'
        skip.className = 'driver-popover-footer-btn theuth-tour-skip'
        skip.textContent = 'Skip tour'
        skip.addEventListener('click', () => dr.destroy())
        popover.footerButtons.prepend(skip)
      },
      onDestroyed: () => {
        if (active === d) active = null
      },
    })
    active = d
    d.drive()
  }
  tick()
}
