import { X } from "lucide-react";
import {
  type CSSProperties,
  type ReactNode,
  useEffect,
  useMemo,
  useState,
} from "react";
import { AppView, preloadAppView } from "@/components/apps/AppView";
import { ArtifactView } from "@/components/artifacts/Viewer";
import { autoOpens } from "@/components/autoresearch/setup";
import { Button } from "@/components/ui/button";
import { ResizeHandle } from "@/components/ui/resize-handle";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Sheet,
  SheetClose,
  SheetContent,
  SheetDescription,
  SheetTitle,
} from "@/components/ui/sheet";
import { fmtBytes } from "@/lib/files";
import { clamp, PX_STEP } from "@/lib/resize-drag";
import { openTileOf, tileable } from "@/lib/tiles";
import type { Artifact } from "@/rpc/bindings/method.artifact.output"
import {
  type AppDraft,
  on,
  useSessionField,
  useSessions,
} from "@/store/sessions";
import { type ArtifactProps, showArtifact } from "@/components/tiles/open";
import { ArtifactMenu, PopoutButton, RevealButton } from "./ArtifactCard";

const MIN_W = 360;
const DEFAULT_W = 640;

/** The newest artifact sharing `a`'s name in its session. */
function headOf(a: Artifact, artifacts: Map<string, Artifact>): Artifact {
  let head = a;
  for (const x of artifacts.values())
    if (x.name === a.name && x.version > head.version) head = x;
  return head;
}

/** A draft's first renderable partial (46 §8.1): a `root` statement for a new app, one complete line of a patch. */
export const renderable = (d: AppDraft): boolean =>
  d.edit ? d.source.includes("\n") : /^\s*root\s*=/m.test(d.source);

/** A patch still arriving or awaiting commit against `shown`'s lineage, to attach to the open view; settled drafts open on their own card. */
function lineageDraft(
  shown: Artifact,
  artifacts: Map<string, Artifact>,
  drafts: Map<string, AppDraft> | undefined,
): AppDraft | undefined {
  if (!drafts) return undefined;
  for (const d of drafts.values()) {
    if (
      !d.edit ||
      !d.base ||
      (d.status !== "streaming" && d.status !== "validating")
    )
      continue;
    if (artifacts.get(d.base)?.name === shown.name) return d;
  }
  return undefined;
}

/** What `ArtifactContent` hands its container: the header's title and meta line, its controls (version, reveal,
 * pop-out, ⋯ menu) and the body. The sheet and the tile (`tiles/kinds/artifact`) each draw their own header. */
export type ArtifactParts = {
  title: string;
  meta: string;
  controls: ReactNode;
  body: ReactNode;
};

/**
 * The artifact (or app draft) at `callId`, as parts for `children` (`null` while neither exists). Opened on the newest
 * version of a name it follows later versions ("latest" mode, 46 §3.3); opened on, or switched to, an older one it
 * stays pinned there. An `app` draft renders through the same `AppView` slot its committed artifact takes, so
 * `ArtifactShown` promotes the open view instead of replacing it; a streaming patch against the shown lineage attaches
 * to that same view (its state and any running action survive) rather than opening a second one. Key it by `callId`.
 */
export function ArtifactContent({
  session,
  callId,
  children,
}: {
  session: string;
  callId: string;
  children: (parts: ArtifactParts | null) => ReactNode;
}) {
  const artifacts = useSessionField(session, "artifacts");
  const drafts = useSessionField(session, "appDrafts");
  const opened = artifacts?.get(callId);
  // `null` = follow the latest version; a `call_id` = pinned.
  const [pinned, setPinned] = useState<string | null>(() =>
    opened && artifacts && headOf(opened, artifacts).call_id !== callId
      ? callId
      : null,
  );
  const shown = pinned
    ? (artifacts?.get(pinned) ?? opened)
    : opened && artifacts
      ? headOf(opened, artifacts)
      : opened;
  const draft =
    drafts?.get(callId) ??
    (shown && !pinned && artifacts
      ? lineageDraft(shown, artifacts, drafts)
      : undefined);
  const versions = useMemo(() => {
    if (!shown || !artifacts) return [];
    return [...artifacts.values()]
      .filter((a) => a.name === shown.name)
      .sort((x, y) => y.version - x.version);
  }, [artifacts, shown]);
  if (!shown && !draft) return children(null);
  const title = shown?.title ?? draft?.title ?? draft?.name ?? "app";
  const meta = shown
    ? `${shown.name} · ${shown.kind.toUpperCase()} · ${fmtBytes(shown.bytes)}`
    : `${draft?.name ?? "app"} · APP · ${draft?.edit ? "patch " : ""}${draft?.status} · ${fmtBytes(draft?.bytes ?? 0)}`;
  const app = draft !== undefined || shown?.kind === "app";
  const controls = shown && (
    <>
      {versions.length > 1 && (
        <Select
          value={shown.call_id}
          onValueChange={(id) =>
            setPinned(id === versions[0]?.call_id ? null : id)
          }
        >
          <SelectTrigger
            size="sm"
            className="h-8 shrink-0 text-xs"
            aria-label="Version"
          >
            <SelectValue />
          </SelectTrigger>
          <SelectContent align="end">
            {versions.map((v) => (
              <SelectItem
                key={v.call_id}
                value={v.call_id}
                className="text-xs"
              >
                v{v.version}
                {v === versions[0] ? " (latest)" : ""}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      )}
      <RevealButton artifact={shown} />
      <PopoutButton artifact={shown} />
      <ArtifactMenu artifact={shown} />
    </>
  );
  const body = app ? (
    <AppView
      session={session}
      artifact={shown?.kind === "app" ? shown : undefined}
      draft={draft}
      className="min-h-0 flex-1 overflow-auto"
    />
  ) : shown ? (
    <ArtifactView
      artifact={shown}
      className="min-h-0 flex-1 overflow-auto"
    />
  ) : null;
  return children({ title, meta, controls, body });
}

/** The sheet's header over the body. */
function SheetParts({ title, meta, controls, body }: ArtifactParts) {
  return (
    <>
      <div className="flex items-start gap-2 border-b border-border-subtle px-4 pt-3 pb-3 pr-3">
        <div className="min-w-0 flex-1">
          <SheetTitle className="truncate text-base">{title}</SheetTitle>
          <SheetDescription className="truncate text-xs">
            {meta}
          </SheetDescription>
        </div>
        {controls}
        <SheetClose asChild>
          <Button
            variant="ghost"
            size="icon-sm"
            className="text-fg-secondary"
            aria-label="Close"
          >
            <X className="size-4" />
          </Button>
        </SheetClose>
      </div>
      {body}
    </>
  );
}

/** `turnSeq` per session of the turn whose first artifact already opened (or was dismissed): auto-open once per turn. */
const handled = new Map<string, number | undefined>();

/** The user closed `session`'s artifact (sheet or tile): no auto-open for the rest of this turn. */
export function dismissArtifact(session: string): void {
  handled.set(session, useSessions.getState().sessions[session]?.turnSeq);
}

/** A deleted chat's once-per-turn mark goes with it (`App`'s `SessionDeleted`). */
export function forgetArtifact(session: string): void {
  handled.delete(session);
}

/** The artifact on screen for `session`: its tile's in tile mode, else the sheet's. */
function onScreen(session: string): string | null | undefined {
  return tileable(session)
    ? (openTileOf("artifact", session)?.props as ArtifactProps | undefined)?.callId
    : useSessions.getState().sessions[session]?.artifactPanel;
}

/**
 * Right-side sheet showing one artifact (35 §7.2) on a phone or outside the panes; where the layout tiles the artifact
 * opens as a tile beside the chat instead (`showArtifact`). Opens itself once per turn while the window is focused: for
 * the first `ArtifactShown`, or earlier for the first renderable partial of an `app` draft (46 §8.1); closing it (sheet
 * or tile) suppresses auto-open for the rest of that turn.
 */
export function ArtifactPanel({ session }: { session: string }) {
  const callId = useSessionField(session, "artifactPanel") ?? null;
  const [width, setWidth] = useState(DEFAULT_W);

  useEffect(() => {
    const open = (id: string) => {
      const turn = useSessions.getState().sessions[session]?.turnSeq;
      if (handled.get(session) === turn) return;
      handled.set(session, turn);
      if (document.hasFocus()) showArtifact(session, id);
    };
    const offShown = on("artifact.shown@1", "ArtifactShown", (e) => {
      if (e.session !== session || !autoOpens(e.artifact.name, window.location.pathname)) return;
      // A new version of the lineage already on screen is adopted (latest mode) or ignored (pinned) by the open
      // `ArtifactContent`; changing the key would remount it and drop its state and any running action.
      const s = useSessions.getState().sessions[session];
      const id = onScreen(session);
      const current = id ? s?.artifacts.get(id) : undefined;
      if (current && current.name === e.artifact.name) return;
      open(e.call_id);
    });
    // Overlap the runtime's cold load with the arguments' arrival: the panel opens on the first renderable partial.
    const offPending = on("agent.events@1", "ToolCallPending", (e) => {
      if (e.session === session && e.name === "app" && document.visibilityState === "visible") preloadAppView();
    });
    const offProgress = on("agent.events@1", "ToolCallProgress", (e) => {
      if (e.session !== session) return;
      const s = useSessions.getState().sessions[session];
      const draft = s?.appDrafts.get(e.call_id);
      if (!s || !draft || !renderable(draft)) return;
      // A patch against a lineage already on screen attaches to that view (`ArtifactContent` picks it up): no remount.
      const base =
        draft.edit && draft.base ? s.artifacts.get(draft.base) : undefined;
      const id = onScreen(session);
      const current = id ? s.artifacts.get(id) : undefined;
      if (base && current && current.name === base.name) return;
      open(base?.call_id ?? e.call_id);
    });
    return () => {
      offShown();
      offProgress();
      offPending();
    };
  }, [session]);

  const close = () => {
    dismissArtifact(session);
    useSessions.getState().setArtifactPanel(session, null);
  };

  // The sheet hangs off the right edge: its width is the distance from the pointer to the window's right side.
  const clampWidth = (w: number) => clamp(w, MIN_W, window.innerWidth / 2);

  return (
    <Sheet open={callId !== null} onOpenChange={(o) => !o && close()}>
      <SheetContent
        showCloseButton={false}
        // No `p-0`: `cn` would let it swallow the sheet's `pt-[var(--safe-top)]` and put the header under the notch.
        className="w-full gap-0 transition-transform sm:w-(--panel-w) sm:max-w-[50vw]"
        style={{ "--panel-w": `${width}px` } as CSSProperties}
      >
        <ResizeHandle
          orientation="vertical"
          placement="start"
          aria-label="Resize artifact panel"
          className="absolute inset-y-0 left-0 hidden w-1.5 bg-transparent hover:bg-border-strong sm:block"
          onMove={(x) => setWidth(clampWidth(window.innerWidth - x))}
          onNudge={(d) => setWidth(clampWidth(width - d * PX_STEP))}
        />
        {callId && (
          <ArtifactContent key={callId} session={session} callId={callId}>
            {(parts) => parts && <SheetParts {...parts} />}
          </ArtifactContent>
        )}
      </SheetContent>
    </Sheet>
  );
}
