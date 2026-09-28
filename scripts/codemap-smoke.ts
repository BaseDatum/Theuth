/**
 * 73 E1–E2 end-to-end smoke against a headless engine:
 *   t --serve --data-dir /tmp/codemap-smoke   (THEUTH_RPC__LISTEN=127.0.0.1:7863)
 *   bun scripts/codemap-smoke.ts --url ws://127.0.0.1:7863 --data-dir /tmp/codemap-smoke --root $PWD
 * One model turn runs `code_map` (→ `CodeToolRan`), then the four `codemap_*` lenses are timed
 * (20 warm calls each for here/fit, change WORKTREE once cold + once cached), then `codemap_touch`
 * on one file (→ `CodeReconciled`). Prints numbers for the as-built.
 */
import { unlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { connect, parseArgs, readToken, recorder } from "./apps/rpc.ts";

const args = parseArgs(process.argv.slice(2));
const url = String(args.flags.url ?? "ws://127.0.0.1:7863");
const dataDir = String(args.flags["data-dir"] ?? "/tmp/codemap-smoke");
const root = String(args.flags.root ?? process.cwd());
const touchPath = String(args.flags.touch ?? "crates/codeindex/src/lens.rs");

const rpc = await connect(url, readToken(dataDir));
const rec = await recorder(rpc, null);

const median = (xs: number[]) => [...xs].sort((a, b) => a - b)[Math.floor(xs.length / 2)]!;
const fmt = (xs: number[]) => `median ${median(xs).toFixed(1)} ms · min ${Math.min(...xs).toFixed(1)} · max ${Math.max(...xs).toFixed(1)}`;

async function timed<T>(method: string, params: unknown[], n: number): Promise<{ last: T; wall: number[]; engine: number[] }> {
  const wall: number[] = [];
  const engine: number[] = [];
  let last!: T;
  for (let i = 0; i < n; i++) {
    const t = performance.now();
    last = await rpc.call<T>(method, params);
    wall.push(performance.now() - t);
    engine.push((last as { meta: { ms: number } }).meta.ms);
  }
  return { last, wall, engine };
}

// 1. A model turn that runs code_map → CodeToolRan on the event channel.
const prompt = `Call code_map(root=${JSON.stringify(root)}, budget=800) exactly once, then reply with the single word done. Use no other tool.`;
const t0 = performance.now();
const session = await rpc.call<string>("send_input", [null, prompt, [], null, null]);
console.log(`session ${session}; waiting for CodeToolRan…`);
const ran = (await rec.waitFor((e) => e.type === "CodeToolRan", 300_000, "CodeToolRan")) as Record<string, unknown>;
console.log(`CodeToolRan after ${((performance.now() - t0) / 1000).toFixed(1)} s:`);
console.log(JSON.stringify({ ...ran, args: ran.args, summary: ran.summary }, null, 0).slice(0, 900));
const engineRoot = String(ran.root);
const firstReconciled = rec.events.find((e) => e.type === "CodeReconciled") as Record<string, unknown> | undefined;
console.log(`CodeReconciled from the first index: ${firstReconciled ? JSON.stringify({ ...firstReconciled, reparsed: `[${(firstReconciled.reparsed as string[]).length}]` }) : "none yet"}`);

// 2. Lenses, warm.
const here = await timed<{ rows: unknown[]; dropped: number; dirs: unknown[]; cache_hit: boolean; personalised: boolean; meta: { ms: number; seq: number } }>(
  "codemap_here", [engineRoot, null, null, null], 20,
);
console.log(`codemap_here ×20: rows ${here.last.rows.length} dropped ${here.last.dropped} dirs ${here.last.dirs.length} cache_hit(last) ${here.last.cache_hit}`);
console.log(`  first call wall ${here.wall[0]!.toFixed(1)} ms (engine ${here.engine[0]} ms); warm ${fmt(here.wall.slice(1))}; engine ms warm ${fmt(here.engine.slice(1))}`);
const hereP = await timed<{ rows: unknown[]; cache_hit: boolean; personalised: boolean }>(
  "codemap_here", [engineRoot, ["crates/codeindex"], ["crates/codeindex/src/lens.rs"], null], 5,
);
console.log(`codemap_here personalised ×5: rows ${hereP.last.rows.length} personalised ${hereP.last.personalised}; cold wall ${hereP.wall[0]!.toFixed(1)} ms (engine ${hereP.engine[0]}); warm ${fmt(hereP.wall.slice(1))}`);

const top = (here.last.rows as { moniker: string; name: string; callers: number }[]).slice().sort((a, b) => b.callers - a.callers)[0]!;
const fit = await timed<{ focus: { name: string; path: string }; callers: unknown[]; callees: unknown[]; edges: unknown[]; more_callers: number; more_callees: number; ambiguous: number; candidates: unknown[] }>(
  "codemap_fit", [engineRoot, top.moniker, null], 20,
);
const f = fit.last;
console.log(`codemap_fit(${top.moniker}) ×20: focus ${f.focus.name} callers ${f.callers.length} (+${f.more_callers}) callees ${f.callees.length} (+${f.more_callees}) edges ${f.edges.length} ambiguous ${f.ambiguous}`);
console.log(`  wall ${fmt(fit.wall)}; engine ms ${fmt(fit.engine)}`);
const byName = await rpc.call<{ focus: { name: string }; candidates: unknown[] }>("codemap_fit", [engineRoot, top.name, null]);
console.log(`codemap_fit(${JSON.stringify(top.name)}) by name: focus ${byName.focus.name}, candidates ${byName.candidates.length}`);

const change = await timed<{ mode: string; range: [string, string] | null; changed: unknown[]; direct: unknown[]; transitive: unknown[]; tests: unknown[]; more: number[]; unindexed: string[]; cache_hit: boolean }>(
  "codemap_change", [engineRoot, null, null, true], 2,
);
const c = change.last;
console.log(`codemap_change(WORKTREE): mode ${c.mode} range ${JSON.stringify(c.range)} bands ${c.changed.length}/${c.direct.length}/${c.transitive.length}/${c.tests.length} more ${JSON.stringify(c.more)} unindexed ${c.unindexed.length}`);
console.log(`  cold wall ${change.wall[0]!.toFixed(1)} ms (engine ${change.engine[0]} ms); cached wall ${change.wall[1]!.toFixed(1)} ms (engine ${change.engine[1]} ms, cache_hit ${c.cache_hit})`);

// 3. touch: an unchanged file is a no-op; a new probe file is re-parsed (→ CodeReconciled); its removal too.
let from = rec.events.length;
let tt = performance.now();
const noop = await rpc.call<{ seq: number; ms: number; stale: boolean }>("codemap_touch", [engineRoot, [touchPath]]);
console.log(`codemap_touch([${touchPath}], unchanged) wall ${(performance.now() - tt).toFixed(1)} ms engine ${noop.ms} ms seq ${noop.seq}`);
const probe = "scripts/codemap-smoke-probe.ts";
writeFileSync(join(root, probe), "export function codemapProbe(): number {\n  return 73;\n}\n");
from = rec.events.length;
tt = performance.now();
const meta = await rpc.call<{ seq: number; ms: number; stale: boolean }>("codemap_touch", [engineRoot, [probe]]);
console.log(`codemap_touch([${probe}], new file) wall ${(performance.now() - tt).toFixed(1)} ms engine ${meta.ms} ms seq ${meta.seq}`);
const reconciled = await rec.waitFor((e) => e.type === "CodeReconciled", 10_000, "CodeReconciled", from);
console.log(`CodeReconciled after touch: ${JSON.stringify(reconciled)}`);
const probeFit = await rpc.call<{ focus: { moniker: string } }>("codemap_fit", [engineRoot, "codemapProbe", null]);
console.log(`codemap_fit(codemapProbe) sees the new definition: ${probeFit.focus.moniker}`);
unlinkSync(join(root, probe));
from = rec.events.length;
await rpc.call("codemap_touch", [engineRoot, [probe]]);
const removed = await rec.waitFor((e) => e.type === "CodeReconciled", 10_000, "CodeReconciled(removed)", from);
console.log(`CodeReconciled after delete: ${JSON.stringify(removed)}`);
try {
  await rpc.call("codemap_touch", [engineRoot, ["../outside.rs"]]);
  console.log("touch outside root: UNEXPECTEDLY accepted");
} catch (e) {
  console.log(`touch outside root rejected: ${(e as Error).message}`);
}
try {
  await rpc.call("codemap_here", ["/tmp", null, null, null]);
  console.log("here on /tmp: UNEXPECTEDLY accepted");
} catch (e) {
  console.log(`here on an unindexed root rejected: ${(e as Error).message}`);
}
await rec.stop();
rpc.close();
