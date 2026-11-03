// Owner: U1
import { useEffect, useState } from 'react'
import { create } from 'zustand'
import { AlertDialog, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import type { Api } from '@/rpc/api'
import type { CellRun } from '@/rpc/bindings/CellRun'
import { KILL_AFTER_MS } from './Cell'
import { duration, labelFor } from './store'

export type QuitSheetProps = {
  /** `cell_live()`: the runs quitting would end. */
  live: CellRun[]
  onQuit(): void
  onCancel(): void
  /** 171:U1-ADD the engine the runs live on: Wait re-reads `cell_live`, Stop sends `cell_stop`. */
  api: Pick<Api, 'cell_live' | 'cell_stop'>
}

/** How often Wait re-reads `cell_live` (and the sheet's clock ticks). */
export const POLL_MS = 1000

const sleep = (ms: number) => {
  const { promise, resolve } = Promise.withResolvers<void>()
  setTimeout(resolve, ms)
  return promise
}

/** **Wait for <label>**: re-read `cell_live` until it is empty (`onLive` gets each read), then resolve; `cancelled()` aborts. */
export async function waitForRuns(api: Pick<Api, 'cell_live'>, onLive: (live: CellRun[]) => void, cancelled: () => boolean): Promise<boolean> {
  for (;;) {
    await sleep(POLL_MS)
    if (cancelled()) return false
    const live = await api.cell_live().catch(() => [])
    onLive(live)
    if (live.length === 0) return true
  }
}

/** **Stop it and quit** (§5.3, W-p): Stop every live run, Kill what is still live `KILL_AFTER_MS` later. */
export async function stopRuns(api: Pick<Api, 'cell_live' | 'cell_stop'>, live: CellRun[]): Promise<void> {
  await Promise.all(live.map((r) => api.cell_stop(r.id, false).catch(() => null)))
  await sleep(KILL_AFTER_MS)
  const still = await api.cell_live().catch(() => [])
  await Promise.all(still.map((r) => api.cell_stop(r.id, true).catch(() => null)))
}

const tail = (cwd: string) => cwd.split('/').filter(Boolean).at(-1) ?? cwd

/** 171 §5.3: the pre-quit sheet listing live cell runs (`App.tsx` quit). */
export function QuitSheet({ live: initial, onQuit, onCancel, api }: QuitSheetProps) {
  const [live, setLive] = useState(initial)
  const [mode, setMode] = useState<'ask' | 'waiting' | 'stopping'>('ask')
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), POLL_MS)
    return () => clearInterval(t)
  }, [])
  useEffect(() => {
    if (mode !== 'waiting') return
    let off = false
    void waitForRuns(api, setLive, () => off).then((done) => done && onQuit())
    return () => {
      off = true
    }
  }, [mode, api, onQuit])
  const first = live[0] ?? initial[0]
  const label = first ? labelFor(first.cell) : 'the cell'
  const what = live.length > 1 ? `${live.length} cells` : label
  return (
    <AlertDialog open onOpenChange={(o) => !o && onCancel()}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{mode === 'waiting' ? `Waiting for ${what}…` : `${what} ${live.length > 1 ? 'are' : 'is'} running`}</AlertDialogTitle>
          <AlertDialogDescription asChild>
            <ul className="flex flex-col gap-1 text-sm">
              {live.map((r) => (
                <li key={r.id} className="tabular-nums">
                  {labelFor(r.cell)} is running ({duration(Math.max(0, (now - (r.started_ms ?? r.queued_ms)) / 1000))}) · {tail(r.cwd)}
                </li>
              ))}
              {mode === 'waiting' && <li className="text-fg-tertiary">theuth quits by itself when the last run ends.</li>}
            </ul>
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <Button variant="outline" onClick={onCancel}>
            Cancel
          </Button>
          <Button
            variant="destructive"
            disabled={mode === 'stopping'}
            onClick={() => {
              setMode('stopping')
              void stopRuns(api, live).then(onQuit)
            }}
          >
            {mode === 'stopping' ? 'Stopping…' : 'Stop it and quit'}
          </Button>
          <Button disabled={mode !== 'ask'} onClick={() => setMode('waiting')}>
            Wait for {what}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}

/** The sheet's place in the quit: `confirmQuit` opens it, the mounted `PreQuit` answers. */
export const usePreQuit = create<{ live: CellRun[] | null; api: QuitSheetProps['api'] | null; answer: ((go: boolean) => void) | null }>(() => ({ live: null, api: null, answer: null }))

/**
 * 171 §5.3 (F6 a, L7): before 163's `requested`, ask `cell_live()`; no live run (or no engine, or an engine without
 * cells) → `true` at once, today's quit. Otherwise the sheet decides: `true` once the runs ended or were stopped,
 * `false` on Cancel. A second quit while the sheet is up answers `false` (the first one is still deciding).
 */
export async function confirmQuit(api: QuitSheetProps['api'] | null): Promise<boolean> {
  if (!api || usePreQuit.getState().answer) return !usePreQuit.getState().answer
  const live = await api.cell_live().catch(() => [])
  if (live.length === 0) return true
  const { promise, resolve } = Promise.withResolvers<boolean>()
  usePreQuit.setState({
    live,
    api,
    answer: (go) => {
      usePreQuit.setState({ live: null, api: null, answer: null })
      resolve(go)
    },
  })
  return promise
}

/** Mounted once in `App.tsx` beside the quit overlay. */
export function PreQuit() {
  const { live, api, answer } = usePreQuit()
  if (!live || !api || !answer) return null
  return <QuitSheet live={live} api={api} onQuit={() => answer(true)} onCancel={() => answer(false)} />
}
