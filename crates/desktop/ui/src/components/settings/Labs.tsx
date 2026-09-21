// 57 §7.1: Settings › Labs — everything experimental about the local decision oracle in one pane: the header's
// "Decision models loaded now" line (`decision_loaded`, 57 §2.1), the meeting oracle and browser System-1 mode
// switches (`decision_mode` / `set_decision_mode`), and per-family training-data Capture (`set_decision_training`)
// and Share (`set_decision_share`) consent. Every control is a live config write; each mode control shows its
// *effective* state. Hidden entirely when `decision.enabled` is false (`useLabsVisible`).

import { useQuery } from '@tanstack/react-query'
import { openPath, openUrl } from '@tauri-apps/plugin-opener'
import { type ReactNode, useState } from 'react'
import { toast } from 'sonner'
import { Empty, Row, Section, errorText, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { fmtBytes, isLocal } from '@/lib/files'
import {
  CAPTURE_ROW,
  MODE_HELP,
  type Prefix,
  SHARE_CONSENT,
  SHARE_UNAVAILABLE,
  blockedReason,
  captureLine,
  countersLine,
  loadedText,
  shareState,
} from '@/lib/labs'
import { queryKeys, useApi } from '@/lib/rpc'
import { useHarnesses } from '@/link/harnesses'
import type { Mode } from '@/rpc/bindings/Mode'

/** The design doc every card links to. */
const DESIGN_NOTES = 'https://github.com/skymoore/wth/blob/main/docs/design/57-decision.md'

/** 57 §7.1: the pane exists only while `decision.enabled` is true (false → no nav entry either). */
export function useLabsVisible(): boolean {
  const api = useApi()
  const info = useQuery({ queryKey: queryKeys.decisionMode('meeting'), queryFn: () => api.decision_mode('meeting') })
  return info.data?.enabled === true
}

export function Labs() {
  const api = useApi()
  const stats = useQuery({ queryKey: queryKeys.decisionStats, queryFn: () => api.decision_stats(), refetchInterval: 30_000 })
  return (
    <>
      <Section title="Loaded now">
        <DecisionLoaded />
      </Section>
      <Section title="Meeting oracle">
        <Blurb>
          A small local classifier (laya, 0.8 GB, on this Mac's GPU) reads each transcript batch before the assistant does: is
          it small talk, does it match a brief you already have? Shadow asks it and shows its verdict beside the model's — nothing
          changes. Active lets a confident verdict skip the model turn or publish the cached brief directly. Costs 100–300 ms
          per batch and roughly 1 GiB of memory while a meeting runs. Nothing leaves this machine.
        </Blurb>
        <ModeRow prefix="meeting" label="Local decision oracle" counters={countersLine(stats.data, 'meeting')} />
      </Section>
      <Section title="Browser System-1">
        <Blurb>
          browser_run hands a goal to a fine-tuned classifier (cklxx/laya-browser) that picks the next click, keystroke or scroll
          from the page's element list — no model turn per step. Shadow decides one step and hands the page back without acting.
          Active performs up to 25 actions (60 s) in the headless browser under one approval. On cklxx's 16-task suite 9 pass
          every run (56 %); a step decides in about 0.6 s on an M3 Max (0.8 s under load). Off ↔ on changes whether the tool is
          offered to the assistant, which takes effect at the next daemon start; Shadow ↔ Active applies live.
        </Blurb>
        <ModeRow prefix="browser" label="Browser System-1" counters={countersLine(stats.data, 'browser')} />
      </Section>
      <Section title="Training data">
        <Blurb>
          Two separate choices per family. <b>Capture</b> writes rows to this machine only, under decisions/training in the data
          directory, secrets replaced by placeholders — nothing is uploaded. <b>Share</b> is consent to upload those same rows
          to theuth for fine-tuning; it needs Capture, and turning Capture off turns Share off. Both are off by default.
        </Blurb>
        <TrainingRows prefix="meeting" title="Meeting" />
        <TrainingRows prefix="browser" title="Browser" />
      </Section>
    </>
  )
}

/** 57 §2.1 item 4: what the daemon holds resident right now, refreshed every 10 s. */
function DecisionLoaded() {
  const api = useApi()
  const loaded = useQuery({ queryKey: queryKeys.decisionLoaded, queryFn: () => api.decision_loaded(), refetchInterval: 10_000 })
  return (
    <Row
      label="Decision models loaded now"
      description="A checkpoint (0.9 GB each) is resident only while a meeting records or a browser session runs the loop; released at stop / close, or after a few idle minutes."
    >
      <span className="text-sm text-fg-secondary" aria-live="polite">
        {loaded.isError ? errorText(loaded.error) : loadedText(loaded.data)}
      </span>
    </Row>
  )
}

/** The card's one-paragraph description with the "design notes" link. */
function Blurb({ children }: { children: ReactNode }) {
  return (
    <p className="mb-2 text-[13px] text-fg-secondary">
      {children}{' '}
      <button type="button" className="underline underline-offset-2 hover:text-fg" onClick={() => void openUrl(DESIGN_NOTES)}>
        design notes
      </button>
    </p>
  )
}

/** Off · Shadow · Active on `decision_mode(prefix)`; disabled with the reason when the oracle cannot run. */
function ModeRow({ prefix, label, counters }: { prefix: Prefix; label: string; counters: string }) {
  const api = useApi()
  const key = queryKeys.decisionMode(prefix)
  const info = useQuery({ queryKey: key, queryFn: () => api.decision_mode(prefix) })
  const set = useAction((mode: Mode) => api.set_decision_mode(prefix, mode), { keys: [key, queryKeys.decisionStats] })
  const mode = info.data?.mode ?? 'off'
  const blocked = blockedReason(info.data, prefix, info.isError ? errorText(info.error) : undefined)
  const help = MODE_HELP[prefix]
  return (
    <Row
      label={label}
      description={
        <>
          {help[mode]}
          {blocked && <span className="block text-danger">Off until this is fixed: {blocked}</span>}
          {prefix === 'browser' && mode === 'active' && !blocked && (
            <span className="block text-danger">The assistant may click, type and scroll in the headless browser for up to 25 steps once you approve a browser_run call.</span>
          )}
          <span className="block text-fg-tertiary">{counters}</span>
        </>
      }
    >
      <ToggleGroup
        type="single"
        variant="outline"
        size="sm"
        value={mode}
        aria-label={label}
        disabled={set.isPending || info.isPending || blocked !== undefined}
        onValueChange={(v) => v && set.mutate(v as Mode)}
      >
        <ToggleGroupItem value="off" title={help.off}>
          Off
        </ToggleGroupItem>
        <ToggleGroupItem value="shadow" title={help.shadow}>
          Shadow
        </ToggleGroupItem>
        <ToggleGroupItem value="active" title={help.active}>
          Active
        </ToggleGroupItem>
      </ToggleGroup>
    </Row>
  )
}

/** One family's Capture and Share rows on `decision_training(prefix)`. */
function TrainingRows({ prefix, title }: { prefix: Prefix; title: string }) {
  const api = useApi()
  const local = isLocal(useHarnesses((s) => s.active))
  const key = queryKeys.decisionTraining(prefix)
  const info = useQuery({ queryKey: key, queryFn: () => api.decision_training(prefix) })
  const capture = useAction((on: boolean) => api.set_decision_training(prefix, on), { keys: [key] })
  const share = useAction((on: boolean) => api.set_decision_share(prefix, on), { keys: [key] })
  const del = useAction(() => api.delete_decision_training(prefix), { keys: [key], onSuccess: () => setConfirm(false) })
  const [confirm, setConfirm] = useState(false)
  const t = info.data
  const shareSwitch = shareState(t)
  const busy = info.isPending || capture.isPending || share.isPending || del.isPending
  if (info.isError) return <Empty>{errorText(info.error)}</Empty>
  return (
    <>
      <Row
        label={`${title} — capture locally`}
        description={
          <>
            {CAPTURE_ROW[prefix]}
            <span className="block text-fg-tertiary">{captureLine(t)}</span>
          </>
        }
      >
        {local && (
          <Button variant="outline" size="sm" disabled={!t || Number(t.bytes) === 0} onClick={() => openPath(t?.dir ?? '').catch((e) => toast.error(errorText(e)))}>
            Open folder
          </Button>
        )}
        {confirm ? (
          <>
            <Button variant="destructive" size="sm" disabled={busy} onClick={() => del.mutate(undefined)}>
              Delete {t?.examples_total ?? 0} examples ({fmtBytes(Number(t?.bytes ?? 0))}) from disk
            </Button>
            <Button variant="ghost" size="sm" onClick={() => setConfirm(false)}>
              Keep
            </Button>
          </>
        ) : (
          <Button variant="outline" size="sm" disabled={!t || Number(t.bytes) === 0} onClick={() => setConfirm(true)}>
            Delete local data
          </Button>
        )}
        <Switch checked={t?.enabled ?? false} disabled={busy} onCheckedChange={(on) => capture.mutate(on)} />
      </Row>
      <Row
        label={<span className="text-fg-secondary">{title} — share with theuth</span>}
        description={
          <>
            {SHARE_CONSENT}{' '}
            <button type="button" className="underline underline-offset-2 hover:text-fg" onClick={() => void openUrl(`${DESIGN_NOTES}#11-training-data-capture-opt-in-per-family-off-by-default`)}>
              privacy note
            </button>
            <span className="block text-fg-tertiary">{SHARE_UNAVAILABLE}{shareSwitch.disabled && t && ' Turn on Capture first.'}</span>
          </>
        }
      >
        <Switch
          className="opacity-60"
          checked={shareSwitch.checked}
          disabled={busy || shareSwitch.disabled}
          onCheckedChange={(on) => share.mutate(on)}
        />
      </Row>
    </>
  )
}
