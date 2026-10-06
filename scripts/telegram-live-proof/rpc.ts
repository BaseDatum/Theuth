#!/usr/bin/env bun
// JSON-RPC over the engine's WebSocket (jsonrpsee).
//
//   bun rpc.ts call <data_dir> <method> [params-json]      → prints the result JSON
//   bun rpc.ts wait <data_dir> [timeout_s]                 → waits for engine.json with a listen addr
//   bun rpc.ts latency <data_dir> <out.csv> [period_s]     → samples telegram_status until killed
//
// Every call has a hard timeout (RPC_TIMEOUT_S, default 10) so a wedged actor
// shows up as a FAIL, never a hang.

import { existsSync, readFileSync, appendFileSync } from "node:fs";
import { join } from "node:path";

const TIMEOUT_S = Number(process.env.RPC_TIMEOUT_S ?? 10);

function endpoint(dataDir: string): string | null {
  const ej = join(dataDir, "engine.json");
  const tok = join(dataDir, "token");
  if (!existsSync(ej) || !existsSync(tok)) return null;
  let rec: { listen?: string | null };
  try {
    rec = JSON.parse(readFileSync(ej, "utf8"));
  } catch {
    return null;
  }
  if (!rec.listen) return null;
  return `ws://${rec.listen}/?token=${readFileSync(tok, "utf8").trim()}`;
}

function connect(url: string, timeoutMs: number): Promise<WebSocket> {
  const { promise, resolve, reject } = Promise.withResolvers<WebSocket>();
  const ws = new WebSocket(url);
  const t = setTimeout(() => {
    ws.close();
    reject(new Error("connect timeout"));
  }, timeoutMs);
  ws.onopen = () => {
    clearTimeout(t);
    resolve(ws);
  };
  ws.onerror = (e) => {
    clearTimeout(t);
    reject(new Error(`ws error: ${(e as ErrorEvent).message ?? "unknown"}`));
  };
  return promise;
}

let nextId = 1;
function call(ws: WebSocket, method: string, params: unknown, timeoutMs: number): Promise<unknown> {
  const id = nextId++;
  const { promise, resolve, reject } = Promise.withResolvers<unknown>();
  const t = setTimeout(() => reject(new Error(`rpc ${method} timed out after ${timeoutMs} ms`)), timeoutMs);
  const onMsg = (ev: MessageEvent) => {
    const m = JSON.parse(String(ev.data));
    if (m.id !== id) return;
    clearTimeout(t);
    ws.removeEventListener("message", onMsg);
    if (m.error) reject(new Error(`rpc ${method}: ${JSON.stringify(m.error)}`));
    else resolve(m.result);
  };
  ws.addEventListener("message", onMsg);
  ws.send(JSON.stringify({ jsonrpc: "2.0", id, method, params: params ?? [] }));
  return promise;
}

const [cmd, dataDir, ...rest] = process.argv.slice(2);
if (!cmd || !dataDir) {
  console.error("usage: rpc.ts call|wait|latency <data_dir> ...");
  process.exit(2);
}

if (cmd === "wait") {
  const deadline = Date.now() + Number(rest[0] ?? 30) * 1000;
  while (Date.now() < deadline) {
    const url = endpoint(dataDir);
    if (url) {
      try {
        const ws = await connect(url, 2000);
        ws.close();
        console.log(url.replace(/token=.*/, "token=…"));
        process.exit(0);
      } catch {}
    }
    await Bun.sleep(200);
  }
  console.error("engine did not come up");
  process.exit(1);
}

if (cmd === "call") {
  const [method, paramsJson] = rest;
  const url = endpoint(dataDir);
  if (!url) {
    console.error("engine.json/token not ready");
    process.exit(1);
  }
  try {
    const ws = await connect(url, TIMEOUT_S * 1000);
    const res = await call(ws, method, paramsJson ? JSON.parse(paramsJson) : [], TIMEOUT_S * 1000);
    ws.close();
    console.log(JSON.stringify(res));
    process.exit(0);
  } catch (e) {
    console.error(String((e as Error).message));
    process.exit(1);
  }
}

if (cmd === "latency") {
  // CSV: epoch_ms,latency_ms,state  — state is the TelegramEvent tag, "ERR:<msg>" on failure.
  const [out, periodS] = rest;
  const period = Number(periodS ?? 2) * 1000;
  let ws: WebSocket | null = null;
  for (;;) {
    const t0 = Date.now();
    let line: string;
    try {
      if (!ws || ws.readyState !== WebSocket.OPEN) {
        const url = endpoint(dataDir);
        if (!url) throw new Error("engine down");
        ws = await connect(url, 2000);
      }
      const st = (await call(ws, "telegram_status", [], 5000)) as { state?: { type: string } | null; polling?: boolean };
      line = `${t0},${Date.now() - t0},${st.state?.type ?? "null"},polling=${st.polling}`;
    } catch (e) {
      line = `${t0},${Date.now() - t0},ERR:${(e as Error).message.replace(/[\n,]/g, " ")},`;
      try {
        ws?.close();
      } catch {}
      ws = null;
    }
    appendFileSync(out, line + "\n");
    const rem = period - (Date.now() - t0);
    if (rem > 0) await Bun.sleep(rem);
  }
}
