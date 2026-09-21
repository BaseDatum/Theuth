// 53 decisions 9–10 / 03 §3: Settings › Meetings — the eye (hidden from capture) with its hotkey, the
// push-to-talk hotkey, and the black-screenshot note. All desktop-local (`lib/meeting-settings.ts`) — except
// the enrolled speaker profiles (04 §7) and the decision-oracle switch (57 §3.4), which are the daemon's.

import { useQuery } from '@tanstack/react-query'
import { ChordInput, Empty, Row, Section, errorText, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { isMacDesktop } from '@/lib/hotkeys'
import { DEFAULT_EYE, DEFAULT_PTT, useMeetingSettings } from '@/lib/meeting-settings'
import { queryKeys, useApi } from '@/lib/rpc'
import { probeShortcut } from '@/lib/shortcuts'
import { setHiddenFromCapture } from '@/lib/tauri'
import type { LoadedInfo } from '@/rpc/bindings/LoadedInfo'
import type { Mode } from '@/rpc/bindings/Mode'

/** 57 §3.4: one line per position. */
export const ORACLE_MODE_HELP: Record<Mode, string> = {
  off: 'Off: every transcript batch is a model turn, as before.',
  shadow: 'Shadow: the oracle is asked and its verdict shown, the model turn still runs; the session rows show whether they agreed.',
  active: 'Active: the oracle gates the turn — a confident "nothing to look up" skips it, a confident cached brief is published directly.',
}

export function Meetings() {
  const api = useApi()
  const hidden = useMeetingSettings((s) => s.hidden)
  const ptt = useMeetingSettings((s) => s.ptt)
  const eye = useMeetingSettings((s) => s.eye)
  const local = isMacDesktop()
  const speakers = useQuery({ queryKey: queryKeys.meetingSpeakers, queryFn: () => api.meeting_speakers() })
  const del = useAction((profile: string) => api.meeting_speaker_delete(profile), { keys: [queryKeys.meetingSpeakers] })
  return (
    <>
      <Section title="Screen capture">
        <Row
          label="Hidden from capture"
          description="Both windows are excluded from screen sharing and screenshots (NSWindow sharingType none). Never automatic: the harness does not detect screen sharing. Your own ⌘⇧3/4 screenshots of the windows come out black while this is on."
        >
          <Switch checked={hidden} disabled={!local} onCheckedChange={(on) => void setHiddenFromCapture(on)} />
        </Row>
        <Row label="Eye hotkey" description="Flips hidden from capture while a meeting records. Click the field and press a chord.">
          <Hotkey kind="eye" value={eye} fallback={DEFAULT_EYE} />
        </Row>
      </Section>
      <Section title="Push-to-talk">
        <Row
          label="Push-to-talk hotkey"
          description="Hold to ask the meeting session a question; what you say while it is held is tagged as a request and answered as a brief. Registered only while a meeting records or is paused."
        >
          <Hotkey kind="ptt" value={ptt} fallback={DEFAULT_PTT} />
        </Row>
      </Section>
      <Section title="Local decision oracle">
        <OracleSwitch />
        <DecisionLoaded />
      </Section>
      <Section title="Enrolled speakers">
        {speakers.isError && <Empty>{errorText(speakers.error)}</Empty>}
        {speakers.data?.length === 0 && <Empty>No enrolled speakers yet. Enroll one from a meeting's review pane.</Empty>}
        {speakers.data?.map((p) => (
          <Row key={p.id} label={p.name} description={`${p.entity ?? '—'} · ${p.samples} sample${p.samples === 1 ? '' : 's'}`}>
            <Button variant="outline" size="sm" disabled={del.isPending} onClick={() => del.mutate(p.id)}>
              Delete
            </Button>
          </Row>
        ))}
      </Section>
    </>
  )
}

/** 57 §3.4 switch: `meeting_oracle_mode` / `set_meeting_oracle_mode`; disabled with the reason when the oracle cannot run. */
function OracleSwitch() {
  const api = useApi()
  const info = useQuery({ queryKey: queryKeys.meetingOracleMode, queryFn: () => api.meeting_oracle_mode() })
  const set = useAction((mode: Mode) => api.set_meeting_oracle_mode(mode), { keys: [queryKeys.meetingOracleMode] })
  const mode = info.data?.mode ?? 'off'
  const blocked = info.data?.reason ?? (info.isError ? errorText(info.error) : undefined)
  return (
    <Row
      label="Local decision oracle"
      description={
        <>
          {ORACLE_MODE_HELP[mode]}
          {blocked && <span className="block text-danger">Off until this is fixed: {blocked}</span>}
          {info.data && !blocked && info.data.effective !== info.data.mode && (
            <span className="block">Running as {info.data.effective} until the next batch.</span>
          )}
        </>
      }
    >
      <ToggleGroup
        type="single"
        variant="outline"
        size="sm"
        value={mode}
        aria-label="Local decision oracle"
        disabled={set.isPending || info.isPending || blocked !== undefined}
        onValueChange={(v) => v && set.mutate(v as Mode)}
      >
        <ToggleGroupItem value="off" title={ORACLE_MODE_HELP.off}>
          Off
        </ToggleGroupItem>
        <ToggleGroupItem value="shadow" title={ORACLE_MODE_HELP.shadow}>
          Shadow
        </ToggleGroupItem>
        <ToggleGroupItem value="active" title={ORACLE_MODE_HELP.active}>
          Active
        </ToggleGroupItem>
      </ToggleGroup>
    </Row>
  )
}

/** 57 §2.1 item 4: what the daemon holds resident right now, refreshed every 10 s. One component so the Labs pane can take it over. */
export function DecisionLoaded() {
  const api = useApi()
  const loaded = useQuery({ queryKey: queryKeys.decisionLoaded, queryFn: () => api.decision_loaded(), refetchInterval: 10_000 })
  return (
    <Row
      label="Decision models loaded now"
      description="A checkpoint is resident only while a meeting records or a browser session runs the loop; released at stop / close, or after a few idle minutes."
    >
      <span className="text-sm text-muted-foreground" aria-live="polite">
        {loaded.isError ? errorText(loaded.error) : loadedText(loaded.data)}
      </span>
    </Row>
  )
}

/** `none` / `meeting · 903 MB · 4 min` (`default` is the meeting checkpoint). */
export function loadedText(infos: LoadedInfo[] | undefined): string {
  if (!infos || infos.length === 0) return 'none'
  return infos
    .map((i) => {
      const name = i.checkpoint === 'default' ? 'meeting' : i.checkpoint
      const mins = Math.floor(i.since_secs / 60)
      const since = mins < 1 ? `${i.since_secs} s` : `${mins} min`
      return `${name} · ${i.rss_mb} MB · ${since}`
    })
    .join(' / ')
}

/** A meeting global shortcut: probed with the plugin (a conflict rejects) before it is saved. */
function Hotkey({ kind, value, fallback }: { kind: 'ptt' | 'eye'; value: string; fallback: string }) {
  const setHotkey = useMeetingSettings((s) => s.setHotkey)
  return (
    <ChordInput
      value={value}
      fallback={fallback}
      commit={(chord) => (chord === fallback ? setHotkey(kind, chord) : probeShortcut(chord).then(() => setHotkey(kind, chord)))}
    />
  )
}
