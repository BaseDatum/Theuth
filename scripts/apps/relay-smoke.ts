/**
 * Apps-over-relay acceptance probe (design 46 + 31): the same app RPC the desktop UI issues,
 * but as a paired *device* through a real `theuth-relay`, using the UI's own Noise IK/IKpsk2
 * transport (`crates/desktop/ui/src/link/*`). Nothing here fakes the relay or the harness.
 *
 * Prerequisites the parent launches (this script starts nothing):
 *
 *   target/debug/theuth-relay --listen 127.0.0.1:18780                       (no --web: no cookie gate)
 *   target/debug/t --headless --data-dir /tmp/theuth-apps-e2e.XXXX          with config.yaml
 *       link: { enabled: true, relay: ws://127.0.0.1:18780 }               and rpc.listen
 *   an app record already published on that harness (smoke.ts `publish` → record_call_id)
 *
 * Run:
 *
 *   bun scripts/apps/relay-smoke.ts --url ws://127.0.0.1:7777 --data-dir /tmp/theuth-apps-e2e.XXXX \
 *     --record-call-id <call_id> [--relay ws://127.0.0.1:18780] [--state /tmp/theuth-apps-e2e.XXXX/argo-state.json] \
 *     [--name relay-smoke] [--timeout-ms 20000] [--keep] [--out result.json]
 *
 * Steps (each an assertion group; the run stops at the first group that cannot continue):
 *   status      `link_status` on the local socket: enabled, uplink connected, relay host matches `--relay`.
 *   pair        `link_pair` → key; a fresh in-memory X25519 identity pairs over the relay (IKpsk2 with the
 *               one-time secret + the enrolling `info` record); `LinkPaired` names our device id; the same
 *               key is refused a second time (single use).
 *   session     IK session handshake over the relay; hello `{name, version}` matches local `info`; `LinkDevice
 *               online` for our device id seen on the local event stream.
 *   get_app     `get_app(record)` over the relay equals the local reply: call_id, version, library_hash, source.
 *   press       `health` load then `app_call_tool` press sync over the relay → succeeded, count+1 (state file too
 *               when `--state`); `app_call_status` over the relay agrees; `app_close` returns null.
 *   reconnect   the device socket is dropped (`transport.reset()`); the client reconnects through the relay
 *               with a new IK handshake, `health` load succeeds again; `LinkDevice offline→online` seen.
 *   revoke      (unless `--keep`) `link_revoke_device` locally → the relay client loses its session and the
 *               device is gone from `link_status`.
 *
 * Output: one JSON document on stdout ({ok, steps[], meta}); exit 1 on any failed assertion. The pairing key
 * (which carries the one-time secret) is never printed; the device private key lives only in this process.
 */
import { readFileSync, writeFileSync } from "node:fs";
import {
  deviceId,
  generateIdentity,
} from "../../crates/desktop/ui/src/link/keys";
import {
  pair,
  parsePairingKey,
} from "../../crates/desktop/ui/src/link/pairing";
import { LinkTransport } from "../../crates/desktop/ui/src/link/transport";
import {
  type ConnState,
  RpcClient,
} from "../../crates/desktop/ui/src/rpc/client";
import fixtures from "./fixtures.json";
import {
  connect,
  parseArgs,
  readToken,
  recorder,
  type Recorder,
  type WireEvent,
} from "./rpc.ts";

interface Device {
  id: string;
  name: string;
  platform: string;
  online: boolean;
}
interface LinkStatus {
  enabled: boolean;
  relay: string;
  name: string;
  harness_id: string | null;
  connected: boolean;
  devices: Device[];
}
interface Pairing {
  key: string;
  expires_at_ms: number;
}
interface Info {
  name: string;
  version: string;
}
interface AppDoc {
  artifact: { call_id: string; version: number; session: string };
  source: string;
  library_hash: string;
}
interface AppToolResult {
  request_id: string;
  structured: Record<string, unknown> | null;
  is_error: boolean;
  status: string;
}
interface Assertion {
  name: string;
  ok: boolean;
  expected?: unknown;
  actual?: unknown;
}
interface Step {
  name: string;
  ok: boolean;
  ms: number;
  assertions: Assertion[];
  error?: string;
}

const { flags } = parseArgs(process.argv.slice(2));
const str = (k: string, d?: string) =>
  typeof flags[k] === "string" ? (flags[k] as string) : d;
const url = str("url", "ws://127.0.0.1:7777")!;
const dataDir = str("data-dir");
const record = str("record-call-id");
const relayUrl = str("relay", "ws://127.0.0.1:18780")!;
const statePath = str("state");
const deviceName = str("name", "relay-smoke")!;
const timeout = Number(str("timeout-ms", "20000"));
const keep = flags.keep === true;
const outPath = str("out");
if (!dataDir || !record) {
  console.error(
    "usage: relay-smoke.ts --url ws://… --data-dir <dir> --record-call-id <id> [--relay ws://127.0.0.1:18780] [--state file] [--keep] [--out file]",
  );
  process.exit(2);
}

const APP_NAME = "portal-web";
const canonical = (v: unknown) =>
  JSON.stringify(v, (_k, x: unknown) =>
    x && typeof x === "object" && !Array.isArray(x)
      ? Object.fromEntries(
          Object.entries(x as Record<string, unknown>).sort(([a], [b]) =>
            a < b ? -1 : a > b ? 1 : 0,
          ),
        )
      : x,
  );
const counts = () => {
  if (!statePath) return undefined;
  try {
    return (
      (
        JSON.parse(readFileSync(statePath, "utf8")) as {
          counts: Record<string, number>;
        }
      ).counts[APP_NAME] ?? 0
    );
  } catch {
    return 0;
  }
};

class Halt extends Error {}
class Check {
  assertions: Assertion[] = [];
  eq(name: string, actual: unknown, expected: unknown) {
    const ok = canonical(actual) === canonical(expected);
    this.assertions.push({ name, ok, expected, actual });
    return ok;
  }
  ok(name: string, ok: boolean, actual?: unknown) {
    this.assertions.push({ name, ok, actual });
    return ok;
  }
  /** Assert, and stop the run when it fails (later steps would only cascade). */
  need(name: string, ok: boolean, actual?: unknown) {
    if (!this.ok(name, ok, actual)) throw new Halt(name);
  }
}

const steps: Step[] = [];
const meta: Record<string, unknown> = {
  url,
  relay: relayUrl,
  record_call_id: record,
  device_name: deviceName,
};
let halted = false;

async function step(name: string, fn: (c: Check) => Promise<void>) {
  if (halted) return;
  const c = new Check();
  const t0 = performance.now();
  const s: Step = { name, ok: true, ms: 0, assertions: c.assertions };
  try {
    await fn(c);
  } catch (e) {
    if (!(e instanceof Halt))
      s.error = e instanceof Error ? e.message : String(e);
    halted = true;
  }
  s.ms = Math.round(performance.now() - t0);
  s.ok = !s.error && c.assertions.every((a) => a.ok);
  steps.push(s);
}

type LinkPaired = WireEvent & { type: "LinkPaired"; device: Device };
type LinkDevice = WireEvent & {
  type: "LinkDevice";
  id: string;
  online: boolean;
};
type LinkRevoked = WireEvent & { type: "LinkRevoked"; id: string };
/** Guards for the link events (31 §6.2) this probe waits on; `id`/`online` narrow the LinkDevice one. */
const isPaired = (e: WireEvent): e is LinkPaired => e.type === "LinkPaired";
const isRevoked = (e: WireEvent): e is LinkRevoked => e.type === "LinkRevoked";
const isDevice =
  (id: string, online: boolean) =>
  (e: WireEvent): e is LinkDevice =>
    e.type === "LinkDevice" && e.id === id && e.online === online;

/** `rec.waitFor` keeping the guard's narrowing; index >= `from`, rejects after `timeout`. */
async function waitEvent<T extends WireEvent>(
  rec: Recorder,
  from: number,
  pred: (e: WireEvent) => e is T,
  what: string,
): Promise<T> {
  return (await rec.waitFor(pred, timeout, what, from)) as T; // waitFor returns the event `pred` accepted
}

/** Next state *transition* of the relay client satisfying `pred` (`onState` first reports the current state; that one is skipped). */
function waitState(
  client: RpcClient,
  pred: (s: ConnState) => boolean,
  what: string,
): Promise<ConnState> {
  const { promise, resolve, reject } = Promise.withResolvers<ConnState>();
  const t = setTimeout(() => {
    off();
    reject(new Error(`timeout waiting for ${what}`));
  }, timeout);
  let initial = true;
  const off = client.onState((s) => {
    if (initial) {
      initial = false;
      return;
    }
    if (!pred(s)) return;
    clearTimeout(t);
    off();
    resolve(s);
  });
  return promise;
}

async function main() {
  const local = await connect(url, readToken(dataDir!));
  const rec = await recorder(local);
  const identity = await generateIdentity(); // non-extractable, in memory only
  const ourId = deviceId(identity.pub);
  meta.device_id = ourId;
  const states: ConnState[] = [];
  let client: RpcClient | undefined;
  let transport: LinkTransport | undefined;
  let localInfo: Info | undefined;

  try {
    await step("status", async (c) => {
      const st = await local.call<LinkStatus>("link_status");
      c.need("link enabled", st.enabled, st.enabled);
      c.need("uplink connected to relay", st.connected, st.connected);
      c.eq("link.relay is the local relay", st.relay, relayUrl);
      c.need(
        "harness has an identity",
        typeof st.harness_id === "string",
        st.harness_id,
      );
      meta.harness_id = st.harness_id;
      meta.harness_name = st.name;
      localInfo = await local.call<Info>("info");
    });

    await step("pair", async (c) => {
      const from = rec.events.length;
      const pairing = await local.call<Pairing>("link_pair");
      const parsed = parsePairingKey(pairing.key);
      c.need("pairing key parses", parsed !== null);
      const expectedHost = new URL(relayUrl).host;
      c.eq("key relay host", parsed!.relayHost, expectedHost);
      c.eq("key harness id", parsed!.harnessId, meta.harness_id);
      c.ok(
        "key not expired",
        pairing.expires_at_ms > Date.now(),
        pairing.expires_at_ms,
      );
      const harness = await pair(pairing.key, deviceName, {
        identity: () => Promise.resolve(identity),
      });
      c.eq("paired harness id", harness.id, meta.harness_id);
      c.eq("PairAccept name", harness.name, meta.harness_name);
      const paired = await waitEvent(rec, from, isPaired, "LinkPaired");
      c.eq(
        "LinkPaired device",
        {
          id: paired.device.id,
          name: paired.device.name,
          platform: paired.device.platform,
        },
        { id: ourId, name: deviceName, platform: "web" },
      );
      // Single use (31 §4.5): the consumed key must not pair a second identity.
      const again = await pair(pairing.key, `${deviceName}-again`, {
        identity: generateIdentity,
      }).then(
        () => "paired",
        (e: Error) => e.message,
      );
      c.eq("consumed key refused", again, "expired or used key");
      c.eq(
        "link_status lists our device",
        (await local.call<LinkStatus>("link_status")).devices.map((d) => d.id),
        [ourId],
      );
      transport = new LinkTransport(harness, {
        identity: () => Promise.resolve(identity),
      });
      client = new RpcClient(transport);
      client.onState((s) => states.push(s));
    });

    await step("session", async (c) => {
      const from = rec.events.length;
      await client!.connect();
      c.eq("hello", transport!.hello, {
        name: localInfo!.name,
        version: localInfo!.version,
      });
      const info = await client!.call<Info>("info", []);
      c.eq(
        "info over relay equals local info",
        { name: info.name, version: info.version },
        { name: localInfo!.name, version: localInfo!.version },
      );
      await waitEvent(rec, from, isDevice(ourId, true), "LinkDevice online");
      c.ok("LinkDevice online", true);
      meta.hello = transport!.hello;
    });

    await step("get_app", async (c) => {
      const [viaRelay, viaLocal] = await Promise.all([
        client!.call<AppDoc>("get_app", [record]),
        local.call<AppDoc>("get_app", [record]),
      ]);
      c.eq("record call_id", viaRelay.artifact.call_id, record);
      c.eq("version", viaRelay.artifact.version, viaLocal.artifact.version);
      c.eq(
        "library_hash equals local",
        viaRelay.library_hash,
        viaLocal.library_hash,
      );
      c.eq("source equals local", viaRelay.source, viaLocal.source);
      c.eq(
        "source is the fixture app",
        viaRelay.source,
        fixtures.app.source.join("\n"),
      );
      c.ok(
        "no secret value in source",
        !viaRelay.source.includes(fixtures.secret.value),
      );
      meta.library_hash = viaRelay.library_hash;
      meta.session = viaRelay.artifact.session;
    });

    await step("press", async (c) => {
      const before = counts();
      const health = await client!.call<AppToolResult>("app_call_tool", [
        callArgs("health", "load"),
      ]);
      c.eq("health load succeeded", health.status, "succeeded");
      const base =
        typeof health.structured?.count === "number"
          ? health.structured.count
          : (before ?? 0);
      if (before !== undefined)
        c.eq(
          "health count equals state file",
          health.structured?.count,
          before,
        );
      const instance_id = `inst-relay-${crypto.randomUUID()}`;
      const request_id = `req-relay-${crypto.randomUUID()}`;
      const press = await client!.call<AppToolResult>("app_call_tool", [
        callArgs("syncResult", "press", {
          instance_id,
          request_id,
          action_id: fixtures.app.actions.sync,
        }),
      ]);
      c.eq("press status", press.status, "succeeded");
      c.eq("press is_error", press.is_error, false);
      c.eq("press count incremented", press.structured?.count, base + 1);
      if (before !== undefined)
        c.eq("state file incremented", counts(), before + 1);
      c.ok(
        "secret value absent from press reply",
        !JSON.stringify(press).includes(fixtures.secret.value),
      );
      const status = await client!.call<AppToolResult>("app_call_status", [
        request_id,
      ]);
      c.eq(
        "app_call_status over relay agrees",
        { status: status.status, request_id: status.request_id },
        { status: "succeeded", request_id },
      );
      c.eq(
        "app_close over relay",
        await client!.call("app_close", [instance_id]),
        null,
      );
    });

    await step("reconnect", async (c) => {
      const from = rec.events.length;
      const helloBefore = transport!.hello;
      const seen = states.length;
      const reopened = waitState(
        client!,
        (s) => s === "open",
        "relay client to reopen",
      );
      const t0 = performance.now();
      transport!.reset(); // drop the device socket: the RpcClient must reconnect through the relay
      await reopened;
      meta.reconnect_ms = Math.round(performance.now() - t0);
      c.eq(
        "went through reconnecting",
        states.slice(seen).includes("reconnecting"),
        true,
      );
      c.ok(
        "new IK handshake produced a hello",
        transport!.hello !== helloBefore && transport!.hello !== null,
        transport!.hello,
      );
      const offline = await waitEvent(
        rec,
        from,
        isDevice(ourId, false),
        "LinkDevice offline",
      );
      await waitEvent(
        rec,
        rec.events.indexOf(offline) + 1,
        isDevice(ourId, true),
        "LinkDevice online again",
      );
      c.ok("harness saw offline then online", true);
      const health = await client!.call<AppToolResult>("app_call_tool", [
        callArgs("health", "load"),
      ]);
      c.eq("health load after reconnect", health.status, "succeeded");
      const dev = (await local.call<LinkStatus>("link_status")).devices.find(
        (d) => d.id === ourId,
      );
      c.eq("link_status shows device online", dev?.online, true);
    });

    if (!keep) {
      await step("revoke", async (c) => {
        const from = rec.events.length;
        const lost = waitState(
          client!,
          (s) => s !== "open",
          "relay client to lose its session",
        );
        c.eq(
          "link_revoke_device",
          await local.call("link_revoke_device", [ourId]),
          null,
        );
        c.eq(
          "LinkRevoked",
          (await waitEvent(rec, from, isRevoked, "LinkRevoked")).id,
          ourId,
        );
        c.ok("relay client lost its session", (await lost) !== "open");
        c.eq(
          "device gone from link_status",
          (await local.call<LinkStatus>("link_status")).devices.some(
            (d) => d.id === ourId,
          ),
          false,
        );
      });
    }
  } finally {
    client?.close();
    await rec.stop().catch(() => undefined);
    local.close();
  }

  meta.states = states;
  meta.event_types = rec.events.reduce<Record<string, number>>(
    (m, e) => ((m[e.type] = (m[e.type] ?? 0) + 1), m),
    {},
  );
  const ok = steps.length > 0 && steps.every((s) => s.ok);
  const doc = { ok, steps, meta };
  if (outPath)
    writeFileSync(
      outPath,
      JSON.stringify({ ...doc, events: rec.events }, null, 2),
    );
  console.log(
    JSON.stringify(doc, null, 2).replaceAll(
      fixtures.secret.value,
      "<redacted:APP_SMOKE>",
    ),
  );
  process.exit(ok ? 0 : 1);
}

function callArgs(
  statement_id: string,
  trigger: "load" | "press",
  over: { instance_id?: string; request_id?: string; action_id?: string } = {},
) {
  return {
    record_call_id: record,
    instance_id: over.instance_id ?? `inst-relay-${crypto.randomUUID()}`,
    generation: 1,
    request_id: over.request_id ?? `req-relay-${crypto.randomUUID()}`,
    args: { name: APP_NAME },
    trigger,
    statement_id,
    ...(over.action_id ? { action_id: over.action_id } : {}),
  };
}

main().catch((e: Error) => {
  console.log(
    JSON.stringify(
      { ok: false, error: e.message, stack: e.stack, steps, meta },
      null,
      2,
    ),
  );
  process.exit(1);
});
