---
name: decide
description: Classify many similar items with the local decision model from a Monty loop — decide_many question shapes, the classify → threshold → act → read-the-tail loop, recipes (mail sort, file sort, ticket triage, relevance pre-filter), the honest cost numbers, caps and pitfalls.
---

# decide: the classifier you program

`decide_many(questions)` runs a local classifier over a list of items and returns one row per
item in the same order. Each row costs milliseconds and zero API tokens. You write the loop: ask
about everything, act on the rows the model is sure about, read the unsure tail yourself.

## When to reach for it

Use it when all three hold:

- **many similar items** — emails, files, tickets, search hits, log lines, rows of a table;
- **a fixed set of labels** you can name up front (or one yes/no statement, or an ordinal scale);
- **a cheap per-item judgement** a reader makes from a glance at the text.

Do **not** use it for anything that needs reasoning, arithmetic, following instructions inside
the item, or reading a long document: it picks among the labels you give it from the first ~512
subwords of the state. For ten items, read them. For a hundred or more, `decide_many` first.

## The loop

```python
res = await decide_many(questions)                      # 1. classify everything
sure   = [r for r in res if r["error"] is None and r["choice"] != "none" and r["confidence"] >= T]
unsure = [r for r in res if r not in sure]
# 2. act on `sure` (move, tag, file — your normal tools, under the normal approval)
# 3. read `unsure` yourself and decide
# 4. with capture on: await decide_label(r["log_id"], the_right_id) for rows you corrected
```

Pick `T` on a sample first: run 50 rows, sort by `confidence`, look at where your own judgement
starts disagreeing. `confidence` is a raw margin between the top two labels until the row says
`calibrated: true` — it is not a probability of being right. Rows with `choice == "none"` fit no
label; treat them as unsure.

## Question shapes

Every question is a dict with a `type`. `state` is the item text.

```python
{"type": "choice", "instructions": "Which folder does this email belong in?",
 "labels": ["invoices and receipts", "recruiting and candidates", "newsletters", "personal", "urgent operations"],
 "ids": ["inv", "rec", "news", "me", "ops"],          # optional, same length; returned as keys
 "state": f"{m.sender}\n{m.subject}\n{m.body[:600]}"}

{"type": "bool", "statement": "This message asks the reader to do something before a deadline.",
 "state": text}                                        # choice is "true" / "false"

{"type": "score", "instructions": "How urgent is this ticket?",
 "levels": ["can wait weeks", "this sprint", "today", "production is down"],
 "state": text}                                        # choice is the level index: "0".."3"
```

Result row: `{"choice": id | "none" | None, "probs": {id: p, …}, "confidence": float,
"calibrated": bool, "truncated": bool, "log_id": str, "error": str | None}`. A row with `error`
has no `choice`; the other rows are unaffected.

Rules that matter:

- **`labels` are what the model reads.** Write them as short descriptive phrases
  (`"invoices and receipts"`), never codes (`"INV"`, `"cat_3"`). Quality drops sharply with
  cryptic labels because the model matches meaning, not tokens.
- **`ids` are your keys** and are never shown to the model. Use them when your downstream code
  wants folder ids, ticket components or enum values. Default: the labels themselves.
- **State goes after the labels and is right-truncated at ~512 subwords** (`truncated: true`).
  Put the discriminating text first — sender, subject, first lines, the path — and slice
  bodies (`body[:600]`). The cap is 4 KB per state; a longer state is an `error` row.
- **`none` is a real answer**: "fits no label". It is always added for `choice` questions; do
  not add it yourself and do not use it as an id.

Call arguments: `questions` (list, ≤ 2,000 per call — loop over slices for more; batching inside
a call is automatic), `checkpoint` (`"gliner"` default, `"default"` for laya), `family`
(`"tool"` default, or a configured `tool.<task>` with its own calibration), `capture`
(`"<task>"`, see below), `budget_ms` (per batch, default 2,000). `decide(question, …)` asks one
question and returns one row.

## Which checkpoint

- `gliner` (default) — GLiNER2.5-Decide. Use it for labels **you** invented: it was trained to
  match arbitrary label phrases against text. 55 ms per row at 72 state tokens → 330 ms at 512,
  flat in the number of labels, on an M3 Max.
- `default` — laya. ~22 ms per row on Metal, but near chance on labels it was not fine-tuned for.
  Use it only for a `tool.<task>` family that has been fine-tuned, or for a quick first pass you
  will verify anyway.

Honest totals: 1,000 × 150-token rows ≈ 2 minutes on gliner, tens of seconds on laya. That is
~1,000× cheaper than reading them, not instant. Say so to the user before a 3,000-row sort.

## Recipes

### Mail into folders

```python
folders = {"inv": "invoices and receipts", "rec": "recruiting and candidates",
           "news": "newsletters and announcements", "me": "personal", "ops": "urgent operations"}
mails = await mail_list(limit=1500)
qs = [{"type": "choice", "instructions": "Which folder does this email belong in?",
       "labels": list(folders.values()), "ids": list(folders),
       "state": f"{m['from']}\n{m['subject']}\n{m['body'][:600]}"} for m in mails]
res = await decide_many(qs, capture="mail")
for m, r in zip(mails, res):
    if r["error"] is None and r["choice"] != "none" and r["confidence"] >= 0.6:
        await mail_move(m["id"], folder=r["choice"])
unsure = [(m, r) for m, r in zip(mails, res) if not (r["error"] is None and r["choice"] != "none" and r["confidence"] >= 0.6)]
# read `unsure`; for each one you file by hand: await decide_label(r["log_id"], folder_id)
```

### Files into folders by content

```python
import os
paths = [...]                                          # from list_files / shell find
def head(p):
    with open(p, "rb") as f: return f.read(1024).decode("utf-8", "replace")
qs = [{"type": "choice", "instructions": "Which project folder does this file belong to?",
       "labels": ["tax and accounting documents", "photos and personal media", "work presentations and specs", "installers and archives"],
       "ids": ["Taxes", "Photos", "Work", "Installers"],
       "state": f"{os.path.basename(p)}\n{head(p)}"} for p in paths]
res = await decide_many(qs)
```

The path alone carries a lot; the first 1 KB settles most text files. Binary files give the model
nothing useful — route by extension yourself and send only the rest.

### Ticket triage by component

```python
components = {"api": "backend API and services", "web": "web frontend and UI", "infra": "infrastructure, deploys and CI", "docs": "documentation"}
qs = [{"type": "choice", "instructions": "Which component owns this issue?",
       "labels": list(components.values()), "ids": list(components),
       "state": f"{t['title']}\n{t['description'][:800]}"} for t in tickets]
res = await decide_many(qs)
```

### "Relevant to this question?" pre-filter

```python
qs = [{"type": "bool", "statement": f"This passage helps answer: {question}",
       "state": passage[:1500]} for passage in passages]
res = await decide_many(qs)
keep = [p for p, r in zip(passages, res) if r["error"] is None and r["choice"] == "true" and r["confidence"] >= 0.5]
```

## Capture and labels

The unsure rows you read are also the training set that makes the next run better. Pass
`capture="<task>"` (for example `capture="mail"`) **only when the user asked for a sorter that
should learn**; every answered row is then written as a training example under
`tool.<task>`, and `decide_label(log_id, id)` records the right key for rows you verified or
corrected. Without `capture`, `decide_label` returns an error. Nothing is uploaded; the rows
live in the data dir.

## Pitfalls

- Passing ids as labels (`"labels": ["INV", "REC"]`) — the model sees codes and guesses.
- A state over 4 KB — error row. Slice it; the first lines decide.
- Writing your own batching — unnecessary; the call batches and keeps your order.
- Treating `confidence` as accuracy — it is a margin, and `calibrated` is `false` until a
  temperature has been fitted for the family. Sample first.
- Expecting reasoning — the model does not read instructions inside the item, does not count,
  does not compare items with each other. It picks a label per item.
- Acting on rows with `error` or `choice == "none"` — they are the unsure tail; read them.
