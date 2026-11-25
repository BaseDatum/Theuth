import { save } from "@tauri-apps/plugin-dialog";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { useNavigate } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { useQuerySupport } from "@/rpc/capabilities";
import { queryKeys } from "@/lib/rpc";
import { SendDialog } from "@/components/library/secrets/SendDialog";
import {
  AppWindow,
  File,
  FileArchive,
  FileX,
  FileCode,
  FileText,
  FolderOpen,
  Image as ImageIcon,
  LayoutGrid,
  MoreHorizontal,
  PanelRight,
  Table,
  Workflow,
  Wrench,
} from "lucide-react";
import {
  memo,
  type MouseEvent,
  type ReactNode,
  useEffect,
  useState,
} from "react";
import { notify as toast } from "@/lib/notify";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useMarkShown } from "@/components/hud/queries";
import { ArtifactWriting, ToolSpinner } from "./Indicators";
import { toolKey } from "./disclosure";
import { disclosureKey, useDisclosure } from "@/lib/tool-runs";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { fmtBytes, isLocal, readText, readTextOnline, resolveFile } from "@/lib/files";
import { resultLine } from "@/lib/describe";
import { canPopout, openPopout } from "@/lib/popout";
import { isPrivate, privateOwner } from "@/lib/private";
import { useApi } from "@/lib/rpc";
import { cn } from "@/lib/utils";
import { useEngines } from "@/link/engines";
import type { Api } from '@/rpc/bindings/client'
import type { Artifact, Kind } from "@/rpc/bindings/method.artifact.output"
import { useConnection } from "@/store/connection";
import { type Entry, type SessionsApi, useSessions } from "@/store/sessions";
import { partialShow } from "@/store/reduce";
import { ChatPicker } from "@/components/notes/ChatPicker";
import { AddToWhiteboard } from "@/components/whiteboard/AddToWhiteboard";
import { cardFromArtifact } from "@/components/whiteboard/place-elsewhere";
import { attachTo, toRef } from "@/lib/attach";
import { Img } from "./Attachments";
import { RemoveButton } from "./MessageAssistant";
import { HtmlPage, capFromArgs } from "./HtmlPage";
import { showArtifact } from "@/components/tiles/open";

type Tool = Extract<Entry, { kind: "tool" }>;

/** Kinds whose bytes are UTF-8 text: previewed as lines, copied as text. */
export const TEXT_KINDS: Record<Kind, boolean> = {
  markdown: true,
  text: true,
  csv: true,
  html: true,
  image: false,
  svg: false,
  pdf: false,
  file: false,
  app: true,
  archive: false,
  mermaid: true,
};

/** One lucide glyph per kind; the card slot, the explorer's thumbs and its kind chips share it. */
export const GLYPH: Record<Kind, typeof File> = {
  markdown: FileText,
  text: FileText,
  csv: Table,
  image: ImageIcon,
  svg: ImageIcon,
  pdf: FileText,
  html: FileCode,
  file: File,
  app: LayoutGrid,
  archive: FileArchive,
  mermaid: Workflow,
};

const fail = (e: unknown) =>
  toast.error(e instanceof Error ? e.message : String(e));

/** Show `callId` (`showArtifact`: a tile beside the chat, else the panel). A finished `show` whose record the store lacks (its `ArtifactShown` was lost on a
 * dead relay socket and nothing re-listed) fetches it with `get_artifact` and applies it as the event would have. */
export async function openShow(
  api: Pick<Api, "get_artifact">,
  session: string,
  callId: string,
  name: string,
): Promise<void> {
  if (!useSessions.getState().sessions[session]?.artifacts.has(callId)) {
    try {
      // A private chat's record is served to its owner naming the session only (51 §4.2).
      const artifact = await api.get_artifact(callId, privateOwner(session));
      useSessions
        .getState()
        .applyEvents([{ type: "ArtifactShown", session, call_id: callId, artifact }]);
    } catch {
      toast.error(`could not open ${name}`);
      return;
    }
  }
  showArtifact(session, callId);
}

/**
 * `list_artifacts` into the slice (`routes/c.$session` on load and on every settle). The map is otherwise fed by live
 * `ArtifactShown`s only, which a relay client that slept through them never sees (a parked socket loses the gap,
 * 25 §2.1). A failed list (an engine without the artifact plugin, a socket still down) keeps the map as it is — never `[]`.
 */
export const relistArtifacts = (
  api: Pick<Api, "list_artifacts">,
  store: SessionsApi,
  session: string,
): Promise<void> =>
  api.list_artifacts(session, null).then(
    (a) => store.getState().setArtifacts(session, a),
    () => undefined,
  );

/** Text of an engine file, `null` while loading (including the wait for a down socket to reopen), `''` on failure.
 * `session` names a private owner (`privateOwner`, 51 §4.2); durable reads leave it out. */
export function useText(path: string | null, session?: string): string | null {
  const api = useApi();
  const client = useConnection((s) => s.client);
  const [text, setText] = useState<string | null>(null);
  useEffect(() => {
    if (!path) return;
    let live = true;
    setText(null);
    readTextOnline(api, client, path, session).then(
      (t) => live && setText(t),
      () => live && setText(""),
    );
    return () => {
      live = false;
    };
  }, [api, client, path, session]);
  return text;
}

/** `report.pdf · PDF · 3 pages · 84 KB`; `rows` when the CSV is parsed. */
export function metaLine(a: Artifact, rows?: number): string {
  const parts = [a.name, a.kind.toUpperCase()];
  if (a.pages != null) parts.push(`${a.pages} page${a.pages === 1 ? "" : "s"}`);
  if (rows != null) parts.push(`${rows} row${rows === 1 ? "" : "s"}`);
  parts.push(fmtBytes(a.bytes));
  return parts.join(" · ");
}

/** The one line under an inline page (120 §3): `title · HTML · 12 KB · v2`. */
export function captionLine(a: Artifact): string {
  const parts = [a.title, a.kind.toUpperCase(), fmtBytes(a.bytes)];
  if (a.version > 1) parts.push(`v${a.version}`);
  return parts.join(" · ");
}

/** Save the blob: a native save dialog + `export_artifact` locally; a browser download of the blob URL elsewhere. */
export async function saveAs(api: Api, a: Artifact, local: boolean) {
  if (local) {
    const dest = await save({ defaultPath: a.name });
    if (!dest) return;
    await api.export_artifact(a.id, dest, privateOwner(a.session));
    toast.success(`Saved ${a.name}`);
    return;
  }
  const url = await resolveFile(api, a.path, useEngines.getState().active, privateOwner(a.session));
  const el = document.createElement("a");
  el.href = url;
  el.download = a.name;
  el.click();
}

/** `⋯` menu: Copy (text kinds), Save as… / Download, Open (local), Open in explorer (unless `explorer`), Copy path. */
export function ArtifactMenu({
  artifact: a,
  className,
  explorer,
}: {
  artifact: Artifact;
  className?: string;
  explorer?: boolean;
}) {
  const api = useApi();
  const navigate = useNavigate();
  const active = useEngines((s) => s.active);
  const local = isLocal(active);
  // 66 §7.8: the card's artifact as a reference, into this session's composer or a picked chat.
  const [attaching, setAttaching] = useState(false);
  // 218: the artifact as a card on a picked whiteboard (pages, images, text as an excerpt, mermaid as a diagram).
  const [boarding, setBoarding] = useState(false);
  // 221 §12.5: Send with Bitwarden… when a Bitwarden connection exists; the dialog sends `artifact:<id>`.
  const [sending, setSending] = useState(false);
  const secrets = useApi();
  const canSend = useQuerySupport(["list_secret_connections", "create_send"]);
  const conns = useQuery({ enabled: canSend, queryKey: queryKeys.secretConnections, queryFn: () => secrets.list_secret_connections() });
  const bitwarden = (conns.data ?? []).some((c) => c.backend === "bw");
  const boardable = a.kind === "html" || a.kind === "image" || a.kind === "svg" || a.kind === "markdown" || a.kind === "text" || a.kind === "csv" || a.kind === "mermaid";
  return (
    <>
    {attaching && <ChatPicker open onOpenChange={setAttaching} item={toRef({ kind: "artifact", artifact: a })} />}
    {sending && <SendDialog open onOpenChange={setSending} initial={{ kind: "file", file: `artifact:${a.id}`, label: `artifact ${a.name}`, bytes: a.bytes }} />}
    {boarding && (
      <AddToWhiteboard
        open
        onOpenChange={setBoarding}
        name={a.title || a.name}
        card={() => cardFromArtifact(a, () => readText(api, a.path, privateOwner(a.session)))}
      />
    )}
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="icon-sm"
          className={cn("text-fg-secondary", className)}
          aria-label="More"
        >
          <MoreHorizontal className="size-4" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-48">
        {TEXT_KINDS[a.kind] && (
          <DropdownMenuItem
            onSelect={() =>
              readText(api, a.path, privateOwner(a.session)).then(
                (t) => navigator.clipboard.writeText(t),
                fail,
              )
            }
          >
            Copy
          </DropdownMenuItem>
        )}
        <DropdownMenuItem onSelect={() => saveAs(api, a, local).catch(fail)}>
          {local ? "Save as…" : "Download"}
        </DropdownMenuItem>
        {local && (
          <DropdownMenuItem onSelect={() => openPath(a.path).catch(fail)}>
            Open
          </DropdownMenuItem>
        )}
        {!explorer && (
          <DropdownMenuItem
            onSelect={() =>
              void navigate({
                to: "/artifacts/$callId",
                params: { callId: a.call_id },
              })
            }
          >
            Open in explorer
          </DropdownMenuItem>
        )}
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => attachTo(a.session, toRef({ kind: "artifact", artifact: a }))}>
          Attach to this chat
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => setAttaching(true)}>Attach to chat…</DropdownMenuItem>
        {boardable && <DropdownMenuItem onSelect={() => setBoarding(true)}>Add to whiteboard…</DropdownMenuItem>}
        {bitwarden && <DropdownMenuItem onSelect={() => setSending(true)}>Send with Bitwarden…</DropdownMenuItem>}
        <DropdownMenuSeparator />
        <DropdownMenuItem
          onSelect={() => void navigator.clipboard.writeText(a.path)}
        >
          Copy path
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
    </>
  );
}

/** Reveal the blob in the OS file manager; local engine only (35 §7.4). */
export function RevealButton({ artifact: a }: { artifact: Artifact }) {
  const active = useEngines((s) => s.active);
  const platform = useConnection((s) => s.info?.platform);
  if (!isLocal(active)) return null;
  const label =
    platform === "macos" ? "Reveal in Finder" : "Show in file manager";
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon-sm"
          className="text-fg-secondary"
          aria-label={label}
          onClick={() => revealItemInDir(a.path).catch(fail)}
        >
          <FolderOpen className="size-4" />
        </Button>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}

/** Open the artifact in its own window (`lib/popout.ts`); `size` matches the toolbar it sits in. Nothing where there are no second windows. */
/** Open `a` in its own window, or `undefined` where that cannot work: no second window here (iOS), or a private
 * tree's artifact (a second window is another peer: its bytes are served to their owner only, 51 §4.2). */
export function artifactPopout(a: Artifact): (() => Promise<void>) | undefined {
  if (!canPopout() || isPrivate(a.session)) return undefined;
  return () => openPopout({ kind: "artifact", callId: a.call_id });
}

export function PopoutButton({
  artifact: a,
  className,
}: {
  artifact: Artifact;
  className?: string;
}) {
  const pop = artifactPopout(a);
  if (!pop) return null;
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon-sm"
          className={cn("text-fg-secondary", className)}
          aria-label="Open in new window"
          onClick={(e) => {
            // Inside a card or a list row that is itself clickable.
            e.preventDefault();
            e.stopPropagation();
            void pop().catch((err: unknown) => console.warn("open_window", err));
          }}
        >
          <AppWindow className="size-4" />
        </Button>
      </TooltipTrigger>
      <TooltipContent>Open in new window</TooltipContent>
    </Tooltip>
  );
}

/** The spinner row while a call's arguments stream (35 §0.7): `writing report.pdf… 3.2 KB` for a `show` carrying
 * `content`, `showing report.pdf…` for one naming a path, `drafting <app>… 3.2 KB` for an `app`, `preparing <tool>… 3.2 KB` otherwise. */
export function progressLabel(
  tool: string,
  p: { name?: string; writing?: boolean; bytes: number },
): string {
  const size = fmtBytes(p.bytes);
  if (tool === "app") return `drafting ${p.name ?? "app"}… ${size}`;
  if (tool !== "show") return `preparing ${tool}… ${size}`;
  if (p.writing)
    return p.name ? `writing ${p.name}… ${size}` : `writing… ${size}`;
  return p.name ? `showing ${p.name}…` : "preparing show…";
}

/** Progress label plus a thin indeterminate bar. */
export function Progress({
  tool,
  progress,
}: {
  tool: string;
  progress: { name?: string; writing?: boolean; bytes: number };
}) {
  return (
    <div className="mt-1 flex w-full flex-col gap-1.5">
      <span className="text-xs text-fg-tertiary">
        {progressLabel(tool, progress)}
      </span>
      <div className="h-0.5 w-full overflow-hidden rounded-full bg-layer-3">
        <div className="h-full w-1/3 animate-indeterminate rounded-full bg-fg-accent" />
      </div>
    </div>
  );
}

/** The 56×72 slot: preview PNG, the image itself, the first 6 lines, or a kind glyph. */
function Preview({ a, text }: { a: Artifact; text: string | null }) {
  const box = "h-[72px] w-14 shrink-0 overflow-hidden rounded-lg bg-layer-2";
  const owner = privateOwner(a.session);
  if (a.preview)
    return (
      <Img path={a.preview} session={owner} className={cn(box, "object-cover object-top")} />
    );
  if (a.kind === "image" || a.kind === "svg")
    return <Img path={a.path} session={owner} className={cn(box, "object-cover")} />;
  if (TEXT_KINDS[a.kind] && a.kind !== "html" && text) {
    return (
      <pre
        className={cn(
          box,
          "p-1.5 font-mono text-[10px] leading-3 whitespace-pre text-fg-tertiary",
        )}
      >
        {text.split("\n", 6).join("\n")}
      </pre>
    );
  }
  const Glyph = GLYPH[a.kind];
  return (
    <div
      className={cn(box, "flex items-center justify-center text-fg-tertiary")}
    >
      <Glyph className="size-6" />
    </div>
  );
}

/** Wrench + `<name>` header row: the tool trace reads the same whatever the body. `extra` sits at its end (197 §8's source control). */
function Header({ name, busy, extra }: { name: string; busy?: boolean; extra?: ReactNode }) {
  return (
    <div className="flex items-center gap-2 font-medium">
      <Wrench className="size-3.5 shrink-0 text-fg-secondary" />
      <span>{name}</span>
      {busy && <ToolSpinner />}
      {extra}
    </div>
  );
}

/** 197 §8, §5.1: the card's own call's source (`tool.context.sources[0]`): `Remove source from context`, or once removed a
 * small tag; the card itself stays either way. A whole row already removed, or a read-only page, shows no control.
 * show/app cards draw it in their header here; a `page_preview` card in `MessageTool`'s plain card. */
export function SourceControl({ tool, session, readOnly }: { tool: Tool; session: string; readOnly?: boolean }) {
  // The piece's own target: its owner is the CALL row, never `context.target` (the response row) (197 §5.1).
  const context = tool.context;
  const piece = context?.sources?.[0];
  if (!context || !piece) return null;
  if (piece.removed)
    return (
      <span className="ml-auto shrink-0 rounded-md bg-layer-2 px-1.5 py-0.5 text-[11px] font-normal text-fg-secondary">
        Source removed from context
      </span>
    );
  if (readOnly || context.removed) return null;
  return (
    <RemoveButton
      session={session}
      target={piece.target}
      active={context.active}
      icon={FileX}
      label="Remove source from context"
      className="-my-3 ml-auto shrink-0 opacity-0 transition-opacity group-hover:opacity-100 focus-visible:opacity-100 pointer-coarse:opacity-100"
    />
  );
}

function PanelButton({ onClick }: { onClick: () => void }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon-sm"
          className="text-fg-secondary"
          aria-label="Open in panel"
          onClick={onClick}
        >
          <PanelRight className="size-4" />
        </Button>
      </TooltipTrigger>
      <TooltipContent>Open in panel</TooltipContent>
    </Tooltip>
  );
}

/** The card frame — the same box as every other tool card (`MessageTool`): wrench + `<name>` header, then the preview
 * slot on the left and the body on the right; `onClick` opens the panel. */
function Frame({
  name,
  children,
  slot,
  onClick,
  busy,
  extra,
}: {
  name: string;
  children: ReactNode;
  slot: ReactNode;
  onClick?: () => void;
  busy?: boolean;
  extra?: ReactNode;
}) {
  const click = (e: MouseEvent<HTMLDivElement>) => {
    // Buttons and menus inside the card keep their own actions.
    if (e.target instanceof Element && e.target.closest("button,a,[role=menu]"))
      return;
    onClick?.();
  };
  return (
    <div
      role={onClick ? "button" : undefined}
      className={cn(
        "my-2 min-w-0 rounded-xl border border-border-subtle bg-layer-1 px-4 py-3 text-sm",
        onClick && "cursor-pointer hover:border-border-strong",
      )}
      onClick={click}
    >
      <Header name={name} busy={busy} extra={extra} />
      <div className="mt-2 flex min-w-0 gap-3">
        {slot}
        <div className="flex min-w-0 flex-1 flex-col gap-1">{children}</div>
      </div>
    </div>
  );
}

const FAILED_SLOT = (
  <div className="flex h-[72px] w-14 shrink-0 items-center justify-center rounded-lg bg-danger-bg text-danger">
    <File className="size-6" />
  </div>
);

/** A `show` or `app` tool card (35 §7.1, 46 §8): writing → compiling → shown, or failed with the first diagnostic. An `app`
 * call's draft opens the panel while it streams; a failed or cancelled draft opens it as readable source. */
export const ArtifactCard = memo(function ArtifactCard({
  tool,
  session,
  readOnly,
}: {
  tool: Tool;
  session: string;
  readOnly?: boolean;
}) {
  const artifact = useSessions((s) =>
    s.sessions[session]?.artifacts.get(tool.callId),
  );
  const progress = useSessions((s) =>
    s.sessions[session]?.progress.get(tool.callId),
  );
  const draft = useSessions((s) =>
    s.sessions[session]?.appDrafts.get(tool.callId),
  );
  const [errors, setErrors] = useDisclosure(disclosureKey(toolKey(tool.callId), "errors"));
  // 104 A2: the card on screen in the open session is the user seeing the shown artifact.
  useMarkShown(artifact ? tool.callId : null);
  // Text kinds fetch their bytes: the 6-line preview slot, or the page itself for `html`.
  const preview = artifact && TEXT_KINDS[artifact.kind] ? artifact.path : null;
  const text = useText(preview, privateOwner(artifact?.session));
  const { result } = tool;
  const api = useApi();
  const open = () =>
    // The toast's name before a record exists: the `name` the call gave, else the base name of its `path`.
    void openShow(api, session, tool.callId, partialShow(JSON.stringify(tool.args ?? {}))?.name ?? tool.name);
  const source = <SourceControl tool={tool} session={session} readOnly={readOnly} />;

  if (artifact?.kind === "html") {
    // 120 §3: the card is the page — borderless at reply width on the transcript background; header row kept, one
    // caption line below, the controls in a hover/focus toolbar at the frame's top-right. The page owns its clicks.
    return (
      <div className="my-2 min-w-0 text-sm">
        <Header name={tool.name} extra={source} />
        <HtmlPage
          callId={tool.callId}
          html={text}
          title={artifact.title}
          cap={capFromArgs(tool.args)}
          className="mt-2"
          toolbar={
            <>
              <RevealButton artifact={artifact} />
              <PanelButton onClick={open} />
              <PopoutButton artifact={artifact} />
              <ArtifactMenu artifact={artifact} />
            </>
          }
        />
        <button
          type="button"
          className="mt-1 block max-w-full truncate text-xs text-fg-secondary hover:underline"
          onClick={open}
        >
          {captionLine(artifact)}
        </button>
      </div>
    );
  }

  if (artifact) {
    const rows =
      artifact.kind === "csv" && text
        ? Math.max(0, text.trimEnd().split("\n").length - 1)
        : undefined;
    return (
      <Frame
        name={tool.name}
        extra={source}
        slot={<Preview a={artifact} text={text} />}
        onClick={open}
      >
        <div className="flex items-start gap-2">
          <div className="min-w-0 flex-1">
            <div className="truncate font-medium">{artifact.title}</div>
            <div className="truncate text-xs text-fg-secondary">
              {metaLine(artifact, rows)}
            </div>
          </div>
          {artifact.version > 1 && (
            <span className="shrink-0 rounded-md bg-layer-2 px-1.5 py-0.5 text-[11px] text-fg-secondary">
              v{artifact.version}
            </span>
          )}
        </div>
        <div className="mt-auto flex items-center justify-end gap-0.5">
          <RevealButton artifact={artifact} />
          <PanelButton onClick={open} />
          <PopoutButton artifact={artifact} />
          <ArtifactMenu artifact={artifact} />
        </div>
      </Frame>
    );
  }

  if (result?.isError) {
    const [first = "", ...rest] = result.output.split("\n");
    return (
      <Frame name={tool.name} slot={FAILED_SLOT} onClick={draft && open} extra={source}>
        <div className="rounded-lg bg-danger-bg px-3 py-2 text-danger">
          <div className="font-mono text-xs wrap-anywhere">
            {resultLine(first)}
          </div>
          {rest.length > 0 && (
            <>
              <button
                type="button"
                className="mt-1 text-xs underline"
                onClick={() => setErrors((v) => !v)}
              >
                {errors ? "Hide errors" : "Show errors"}
              </button>
              {errors && (
                <pre className="mt-1 font-mono text-xs whitespace-pre-wrap">
                  {rest.join("\n")}
                </pre>
              )}
            </>
          )}
          {draft && (
            <div className="mt-1 text-xs text-fg-secondary">
              Click to read the {draft.edit ? "patch" : "source"} in the panel.
            </div>
          )}
        </div>
      </Frame>
    );
  }

  // Writing (arguments streaming) or compiling (started, `ArtifactShown` not yet); a finished call whose artifact the list did not carry shows its receipt.
  return (
    <Frame
      name={tool.name}
      extra={source}
      slot={ArtifactWriting}
      busy={!result}
      onClick={draft || (result && !result.isError) ? open : undefined}
    >
      {result ? (
        <div className="truncate font-mono text-xs text-fg-secondary">
          {resultLine(result.output)}
        </div>
      ) : tool.pending ? (
        <Progress tool={tool.name} progress={progress ?? { bytes: 0 }} />
      ) : (
        <>
          <div className="h-4 w-1/2 animate-pulse rounded bg-layer-2" />
          <span className="text-xs text-fg-tertiary">compiling…</span>
        </>
      )}
    </Frame>
  );
});
