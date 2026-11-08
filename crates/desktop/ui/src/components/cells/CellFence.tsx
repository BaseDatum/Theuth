// Owner: U1
import { useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { createContext, useContext, useEffect, useId, useMemo, useState } from 'react'
import { CodeBlock, CodeBlockCopyButton, CodeBlockDownloadButton, type CustomRendererProps, StreamdownContext } from 'streamdown'
import { FenceContext } from '@/components/chat/surface/JsonRenderFence'
import { useApi } from '@/lib/rpc'
import { disclosureKey } from '@/lib/tool-runs'
import type { CellRef } from '@/rpc/bindings/CellRef'
import type { CellRunMode } from '@/rpc/bindings/CellRunMode'
import { Cell, CellExtras } from './Cell'
import { CellEditor } from './CellEditor'
import { setEdit, useEdit } from './edits'
import { bodyKey, type ReplyCell, replyCells } from './fences'
import { cellOf, digest } from './grammar'
import { openAsNote } from './open-as-note'
import { cellIdentity } from './line-run'
import { followSuperseded, RunbookBar, runAllPairs } from './RunbookBar'
import { errorText, lastPass, mintId, press, refKey, refresh, register, registeredCells, scopeKey, scopeOf, setNotice, useCellRecord, useScope, useStates } from './store'
import { chatEditable, disarmedReason, labelOf, RUN_ANYWAY, titleOf, viewOf } from './view'

/** 171 §4.1: the fence languages `CellFence` registers for. */
export const CELL_LANGUAGES = ['bash', 'zsh', 'sh', 'pwsh', 'powershell', 'python', 'py']

/**
 * 171 §3.4, §4.1: what `MessageAssistant` knows about its reply's cells. `text` is the reply's markdown; `entry` the
 * stored message (its seq), `null` until the reply is stored; `cells` the reply's cells in document order
 * (`replyCells`); `claim` hands a mounted fence its ordinal — the first unclaimed cell with the same body, so two
 * identical cells still get two ordinals; the claims reset with each value (one per message text). Only assistant
 * markdown provides it: without it a fence stays code (harness cards, tool results, peer text — §12.2).
 */
export type CellOrdinals = { text: string; entry: string | null; turnRunning: boolean; cells: ReplyCell[]; claim(fence: string, body: string): number | null }
export const CellOrdinals = createContext<CellOrdinals | null>(null)

/** The `CellOrdinals` value for one message text. */
export function cellOrdinals(text: string, entry: string | null, turnRunning: boolean): CellOrdinals {
  const cells = replyCells(text)
  const claims = new Map<string, number>()
  return {
    text,
    entry,
    turnRunning,
    cells,
    claim(fence, body) {
      const known = claims.get(fence)
      if (known !== undefined) return known
      const taken = new Set(claims.values())
      const hit = cells.find((c) => c.body === body && !taken.has(c.ordinal))
      if (!hit) return null
      claims.set(fence, hit.ordinal)
      return hit.ordinal
    },
  }
}

const b64 = (s: string) => btoa(Array.from(new TextEncoder().encode(s), (b) => String.fromCharCode(b)).join(''))

/** streamdown's own code block with the controls its default path mounts (copy, download), honouring `controls`. */
function Snippet({ code, language, isIncomplete, meta }: CustomRendererProps) {
  const { controls, lineNumbers } = useContext(StreamdownContext)
  const c = typeof controls === 'boolean' ? controls : controls.code
  const on = (k: 'copy' | 'download') => (typeof c === 'object' ? c[k] !== false : c !== false)
  const start = Number.parseInt(meta?.match(/startLine=(\d+)/)?.[1] ?? '', 10)
  return (
    <CodeBlock
      code={code}
      className={`language-${language}`}
      language={language}
      isIncomplete={isIncomplete}
      lineNumbers={!/\bnoLineNumbers\b/.test(meta ?? '') && lineNumbers !== false}
      startLine={start >= 1 ? start : undefined}
    >
      {c !== false ? (
        <>
          {on('download') ? <CodeBlockDownloadButton code={code} language={language} /> : null}
          {on('copy') ? <CodeBlockCopyButton /> : null}
        </>
      ) : null}
    </CodeBlock>
  )
}

/** A cell of a reply: binds the shared `Cell` to the chat RPCs (§15.2). */
function ChatCell({ code, language, isIncomplete, ordinals, ordinal }: CustomRendererProps & { ordinals: CellOrdinals; ordinal: number }) {
  const api = useApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const fence = useContext(FenceContext)
  const spec = useMemo(() => cellOf(language, code), [language, code])
  const session = fence.session
  const cellRef = useMemo<CellRef | null>(
    () => (session && ordinals.entry ? { type: 'chat', session, entry: ordinals.entry, ordinal } : null),
    [session, ordinals.entry, ordinal],
  )
  const pending = !!session && (isIncomplete || ordinals.turnRunning || !ordinals.entry)
  // F4: the user's edit outlives a remount (`edits`, by `refKey`); a cell without a ref cannot be edited.
  const editKey = cellRef ? refKey(cellRef) : ''
  const edited = useEdit(editKey)
  const [hint, setHint] = useState<string | null>(null)
  const label = spec ? labelOf(spec) : 'Run'
  const title = spec ? titleOf(spec, ordinal) : ''
  useEffect(() => (cellRef && spec && !pending ? register(api, scopeOf(cellRef), { ref: cellRef, spec, label: title }) : undefined), [api, cellRef, spec, title, pending])
  useEffect(() => {
    if (!hint) return
    const t = setTimeout(() => setHint(null), 4000)
    return () => clearTimeout(t)
  }, [hint])

  const { state, notice } = useCellRecord(cellRef)
  const scope = useScope(cellRef ? scopeOf(cellRef) : { type: 'session', id: '' })
  const replyRefs = useMemo(() => (session && ordinals.entry ? ordinals.cells.map((c): CellRef => ({ type: 'chat', session, entry: ordinals.entry as string, ordinal: c.ordinal })) : []), [session, ordinals])
  const replyStates = useStates(replyRefs)
  // 171F B1: the bar sits after the reply's last cell; B8: it follows replacements, watching them when nothing mounts them.
  const barCell = ordinal === ordinals.cells.length && ordinals.cells.length >= 2 && !!cellRef
  const nextRefs = useMemo(() => (barCell ? replyStates.map((s) => s?.superseded_by ?? null) : []), [barCell, replyStates])
  const nextStates = useStates(nextRefs)
  const nextKey = nextRefs.map((r) => (r ? refKey(r) : '')).join(' ')
  useEffect(() => {
    const offs = nextRefs.flatMap((r, i) => (r ? [register(api, scopeOf(r), { ref: r, spec: null, label: titleOf((ordinals.cells[i] as ReplyCell).spec, i + 1) })] : []))
    return () => offs.forEach((off) => off())
    // `nextKey` names the refs; the ref objects are new on every read.
  }, [api, nextKey])
  if (!spec) return null
  const latest = state?.latest ?? null
  const host = state?.host ?? null
  const view = viewOf(state, spec, edited !== null)
  const runnable = !!cellRef && !pending
  // CELLAFTER (§7.6): the unarmed gate's reason; the button and a second ► run anyway, the keys hint.
  const gate = disarmedReason(state, spec)

  // §11.1 fold: the latest cell of the reply that has run stays open.
  const lastRun = replyStates.reduce((best, s, i) => (s?.latest ? i + 1 : best), 0)
  const after = spec.options.after
  const afterLabel = after
    ? (ordinals.cells.find((c) => c.spec.options.name === after)?.spec.options.label ??
      (cellRef && registeredCells(scopeOf(cellRef)).find((c) => c.spec?.options.name === after)?.label) ??
      undefined)
    : undefined
  const queued = Object.values(scope.states)
    .map((s) => s.latest)
    .filter((r) => r?.state === 'queued' && latest && JSON.stringify(r.kernel) === JSON.stringify(latest.kernel))
    .sort((a, b) => (a?.queued_ms ?? 0) - (b?.queued_ms ?? 0))
  const queuePos = latest?.state === 'queued' ? queued.findIndex((r) => r?.id === latest.id) + 1 : undefined

  const act = (f: () => Promise<unknown>) => {
    if (!cellRef) return
    setNotice(cellRef, null)
    f().then(
      () => refresh(scopeOf(cellRef)),
      (e: unknown) => {
        setNotice(cellRef, errorText(e))
        void refresh(scopeOf(cellRef))
      },
    )
  }
  // `confirmed` only from the presses past a warning: Run anyway, the second ►, the danger confirm (or its "Don't ask again").
  // `at`: the code the press was made on (a danger ask's press-time code, M1), so what was confirmed is what runs.
  const run = (mode: CellRunMode, next?: boolean, confirmed = false, at?: string) => {
    if (!cellRef) return
    const text = at === undefined ? edited : at === spec.code ? null : at
    // `seen` (§3.4, D3): the digest of the text on screen; the engine refuses a mismatch with "this cell changed".
    const pressed = (async () => {
      const seen = await digest(code)
      return press(mintId('r_'), (id) => api.cell_run(cellRef, seen, mode, id, text, null, confirmed))
    })()
    act(() => pressed)
    if (next) {
      // §11.7 Shift+Enter: focus moves to the next cell.
      const all = [...document.querySelectorAll<HTMLElement>('[data-cell] .cm-content')]
      const here = document.activeElement?.closest('[data-cell]')
      all[all.findIndex((el) => el.closest('[data-cell]') === here) + 1]?.focus()
    }
    // 171F4 T2-6: the editor's line/selection mark settles on the run this resolves to.
    return pressed
  }
  const callbacks = {
    // The edited text rides the component's own edit state (F4).
    onRun: (mode: CellRunMode, confirmed?: boolean) => run(mode, false, confirmed),
    onStop: (kill: boolean) => {
      if (latest) act(() => api.cell_stop(latest.id, kill))
    },
    onInput: (data: string) => {
      if (latest) act(() => api.cell_input(latest.id, b64(data)))
    },
    onAsk: () => {
      if (latest) act(() => api.cell_ask(latest.id))
    },
    onEdit: (next: string) => cellRef && setEdit(editKey, next, spec.code),
    // `Terminal`'s cell source attaches itself (one attach path for chat and notes); kept for the frozen `CellCallbacks`.
    onAttach: () => () => undefined,
  }
  const locked = !chatEditable(view, runnable)
  const extras: CellExtras = {
    open: lastRun === ordinal || view === 'failed',
    afterLabel,
    queuePos,
    notice,
    hint,
    pending,
  }
  const steps = barCell
    ? followSuperseded(
        ordinals.cells.map((c, i) => ({ ref: replyRefs[i] as CellRef, state: replyStates[i] ?? null, spec: c.spec })),
        nextStates,
        (r) => registeredCells(scopeOf(r)).find((c) => refKey(c.ref) === refKey(r))?.spec ?? null,
      )
    : []
  const bar =
    barCell && cellRef ? (
      <RunbookBar
        cells={steps}
        kernels={scope.kernels}
        pass={lastPass(scope)}
        whyKey={disclosureKey(scopeKey(scopeOf(cellRef)), ordinals.entry, 'runbook-why')}
        runnable={!pending}
        onRunAll={() => {
          const ref = cellRef
          // W-r: one engine pass over the reply's cells in order; the engine stops before its boundaries (§7.2).
          void (async () => {
            const pairs = await runAllPairs(steps, ordinals.cells.map((c) => c.body))
            await press(mintId('p_'), (id) => api.cell_run_all(scopeOf(ref), pairs, id))
          })().then(
            () => refresh(scopeOf(ref)),
            (e: unknown) => setNotice(ref, errorText(e)),
          )
        }}
        onOpenAsNote={() => void openAsNote(api, qc, session as string, ordinals, replyStates, (id) => void navigate({ to: '/notes/$id', params: { id } }))}
      />
    ) : null
  return (
    <>
      <CellExtras value={extras}>
        <Cell
          cellRef={cellRef ?? { type: 'chat', session: session ?? '', entry: ordinals.entry ?? '', ordinal }}
          cell={spec}
          code={code}
          runnable={runnable}
          view={view}
          state={state}
          latest={latest}
          host={host ? { engine: host, label: host } : null}
          callbacks={callbacks}
          readOnly={!session}
          editor={
            <CellEditor
              lang={spec.lang}
              code={edited ?? spec.code}
              runnable={runnable}
              disarmed={gate}
              title={title}
              identity={cellRef ? cellIdentity(cellRef, spec) : ''}
              latest={latest}
              danger={spec.options.danger}
              locked={locked}
              onChange={callbacks.onEdit}
              onRun={run}
              onRefused={() => setHint(`press ${gate !== null ? RUN_ANYWAY : label} to run this`)}
            />
          }
        />
      </CellExtras>
      {bar}
    </>
  )
}

/**
 * 171 §4.1: a streamdown renderer for shell and python fences. No `#|` header → streamdown's own `CodeBlock` with the
 * controls its default path mounts, so plain snippets look as they always did; a header inside an assistant reply
 * (`CellOrdinals` provided) → the shared `Cell`; anywhere else the fence stays code.
 */
export function CellFence(props: CustomRendererProps) {
  const ordinals = useContext(CellOrdinals)
  const id = useId()
  const header = cellOf(props.language, props.code) !== null
  const ordinal = header && ordinals ? ordinals.claim(id, bodyKey(props.code)) : null
  if (!ordinals || ordinal === null) return <Snippet {...props} />
  return <ChatCell {...props} ordinals={ordinals} ordinal={ordinal} />
}
