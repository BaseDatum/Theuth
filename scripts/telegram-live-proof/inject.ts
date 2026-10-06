#!/usr/bin/env bun
// telegram-mock-ai Admin API driver (patched mock: see mock.patch).
//
//   bun inject.ts message <text>                  → owner sends <text> in the private chat; prints the Message
//   bun inject.ts callback <message_id> <data>    → owner presses a button on <message_id>; prints the callback id
//   bun inject.ts seq                             → highest ledger seq so far (0 when empty)
//   bun inject.ts await <m1,m2,…> <since> <timeout_s> [--chat <id>] [--match <regex>]
//                                                 → first ledger call after <since> whose method is in the list
//                                                   (and whose params JSON matches --match); exit 1 on timeout
//   bun inject.ts calls <since> [--all]           → ledger entries (getUpdates filtered unless --all)
//   bun inject.ts messages                        → stored messages of the private chat
//   bun inject.ts ledger <since> <file> [--all]   → write a one-line-per-call ledger excerpt to <file>
//
// Env: MOCK_ADMIN (http://127.0.0.1:8082), BOT_TOKEN, OWNER (777000111), CHAT (= OWNER).

const ADMIN = process.env.MOCK_ADMIN ?? "http://127.0.0.1:8082";
const TOKEN = process.env.BOT_TOKEN ?? "123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11";
const OWNER = Number(process.env.OWNER ?? 777000111);
const CHAT = Number(process.env.CHAT ?? OWNER);

type Entry = {
  seq: number;
  at: string;
  method: string;
  params?: unknown;
  status: number;
  response?: unknown;
  took_ms: number;
};

async function api(method: string, path: string, body?: unknown): Promise<unknown> {
  const r = await fetch(ADMIN + path, {
    method,
    headers: { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await r.text();
  if (!r.ok) throw new Error(`${method} ${path} → ${r.status} ${text}`);
  return text ? JSON.parse(text) : null;
}

async function calls(since: number, all: boolean): Promise<Entry[]> {
  const es = (await api("GET", `/api/calls?since=${since}`)) as Entry[];
  return all ? es : es.filter((e) => e.method.toLowerCase() !== "getupdates");
}

function flag(args: string[], name: string): string | undefined {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : undefined;
}

// One line per call: seq, time, method, status, took, params (compact, truncated).
function line(e: Entry): string {
  const p = e.params === undefined ? "" : JSON.stringify(e.params);
  const resp = e.status === 200 ? "" : ` resp=${JSON.stringify(e.response ?? "")}`;
  return `${String(e.seq).padStart(4)} ${e.at.slice(11, 23)} ${e.method.padEnd(22)} ${e.status} ${String(e.took_ms).padStart(5)}ms ${p.length > 300 ? p.slice(0, 300) + "…" : p}${resp}`;
}

const [cmd, ...args] = process.argv.slice(2);
try {
  switch (cmd) {
    case "message": {
      const msg = await api("POST", `/api/chats/${CHAT}/messages`, { user_id: OWNER, text: args[0] });
      console.log(JSON.stringify(msg));
      break;
    }
    case "callback": {
      const messageId = Number(args[0]);
      const data = args[1];
      const msgs = (await api("GET", `/api/chats/${CHAT}/messages?limit=500`)) as { message_id: number }[];
      const message = msgs.find((m) => m.message_id === messageId);
      if (!message) throw new Error(`message ${messageId} not in the mock store`);
      const id = String(Date.now());
      const user = ((await api("GET", "/api/users")) as { id: number }[]).find((u) => u.id === OWNER);
      await api("POST", `/api/bots/${TOKEN}/updates`, {
        callback_query: { id, from: user, message, chat_instance: String(CHAT), data },
      });
      console.log(id);
      break;
    }
    case "seq": {
      const es = (await api("GET", "/api/calls?since=0")) as Entry[];
      console.log(es.length ? es[es.length - 1].seq : 0);
      break;
    }
    case "await": {
      const methods = args[0].toLowerCase().split(",");
      const since = Number(args[1]) + 1;
      const deadline = Date.now() + Number(args[2]) * 1000;
      const chat = flag(args, "--chat");
      const match = flag(args, "--match");
      const re = match ? new RegExp(match) : null;
      const t0 = Date.now();
      for (;;) {
        for (const e of await calls(since, false)) {
          if (!methods.includes(e.method.toLowerCase())) continue;
          const p = JSON.stringify(e.params ?? {});
          if (chat && !new RegExp(`"chat_id":"?${chat}"?`).test(p)) continue;
          if (re && !re.test(p)) continue;
          console.log(JSON.stringify({ waited_ms: Date.now() - t0, ...e }));
          process.exit(0);
        }
        if (Date.now() >= deadline) {
          console.error(`timeout after ${Date.now() - t0} ms waiting for ${args[0]}`);
          process.exit(1);
        }
        await Bun.sleep(200);
      }
    }
    case "calls": {
      console.log(JSON.stringify(await calls(Number(args[0] ?? 0), args.includes("--all"))));
      break;
    }
    case "messages": {
      console.log(JSON.stringify(await api("GET", `/api/chats/${CHAT}/messages?limit=500`)));
      break;
    }
    case "ledger": {
      const es = await calls(Number(args[0] ?? 0), args.includes("--all"));
      await Bun.write(args[1], es.map(line).join("\n") + "\n");
      console.log(`${es.length} calls → ${args[1]}`);
      break;
    }
    default:
      console.error("usage: inject.ts message|callback|seq|await|calls|messages|ledger …");
      process.exit(2);
  }
} catch (e) {
  console.error(String((e as Error).message));
  process.exit(1);
}
