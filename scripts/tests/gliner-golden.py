#!/usr/bin/env python3
# /// script
# requires-python = ">=3.10"
# dependencies = ["gliner2[local]==2.0.0", "tokenizers", "huggingface_hub"]
# ///
"""gliner-golden — golden token fixtures for `crates/decision/src/gliner/prompt.rs` (GLINER-2).

Runs the reference preprocessing of the Python `gliner2` package (PyPI 2.0.0 ==
fastino-ai/GLiNER2 @ 55656fb) for `fastino/GLiNER2.5-Decide` (HF revision 5a7adf72) over a
fixed set of theuth `Question`s and writes `crates/decision/src/gliner/fixtures/golden.json`:
per case the Question JSON, gliner2's exact `input_ids`, `attention_mask`, the `[L]` marker
positions and the decoded string. The Rust test `gliner::prompt::tests::golden` reproduces
every case byte-for-byte.

Only the tokenizer is needed: `tokenizer.json`, `tokenizer_config.json`, `config.json` are
read from `$THEUTH_MODELS_DIR/decision-gliner/` (default `~/.theuth/models/decision-gliner/`)
and downloaded from Hugging Face at the pinned revision when absent — never `model.safetensors`.

Question → gliner2 classification task (the same mapping as `prompt.rs::labels`):
  Choice {instructions, candidates, state} → task = instructions, labels = candidate `text`
      (the id when `text` is empty), text = state. `none` is a candidate the Engine appended
      (`engine::with_none`: id "none", text "none of these") before the question reached here.
  Bool {statement, state}                 → task = statement, labels = ["yes", "no"].
  Score {instructions, levels, state}     → task = instructions, labels = levels.

    uv run scripts/tests/gliner-golden.py            # regenerate golden.json
    uv run scripts/tests/gliner-golden.py --check    # regenerate in memory, diff, exit 1 on drift
"""
from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

REPO = "fastino/GLiNER2.5-Decide"
REVISION = "5a7adf72"
FILES = ("tokenizer.json", "tokenizer_config.json", "config.json")
ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates/decision/src/gliner/fixtures/golden.json"

# [SEP_STRUCT] [SEP_TEXT] [P] [C] [E] [R] [L] [EXAMPLE] [OUTPUT] [DESCRIPTION]
MARKERS = {
    "[SEP_STRUCT]": 128001, "[SEP_TEXT]": 128002, "[P]": 128003, "[C]": 128004, "[E]": 128005,
    "[R]": 128006, "[L]": 128007, "[EXAMPLE]": 128008, "[OUTPUT]": 128009, "[DESCRIPTION]": 128010,
}


def model_dir() -> Path:
    base = os.environ.get("THEUTH_MODELS_DIR") or os.path.expanduser("~/.theuth/models")
    d = Path(base) / "decision-gliner"
    d.mkdir(parents=True, exist_ok=True)
    missing = [f for f in FILES if not (d / f).exists()]
    if missing:
        from huggingface_hub import hf_hub_download

        for f in missing:
            hf_hub_download(REPO, f, revision=REVISION, local_dir=str(d))
    return d


def choice(instructions, cands, state):
    return {"type": "choice", "instructions": instructions,
            "candidates": [{"id": i, "text": t} for i, t in cands], "state": state}


def with_none(q):
    if not any(c["id"] == "none" for c in q["candidates"]):
        q["candidates"].append({"id": "none", "text": "none of these"})
    return q


def labels(q) -> list[str]:
    t = q["type"]
    if t == "choice":
        return [c["text"] or c["id"] for c in q["candidates"]]
    if t == "bool":
        return ["yes", "no"]
    return list(q["levels"])


def task_and_text(q) -> tuple[str, str]:
    t = q["type"]
    return (q["statement"] if t == "bool" else q["instructions"]), q["state"]


LONG_STATE = " ".join(
    f"line {i}: the quick brown fox jumps over the lazy dog near warehouse {i % 7}" for i in range(60)
)
STATE_48 = "User typed 'open the settings page' in the command palette while on the dashboard."


def cases() -> list[tuple[str, dict]]:
    return [
        ("choice_2", with_none(choice(
            "Which action does the user want?",
            [("e3", "Sign in button"), ("e7", "Search field")],
            "ref=e3 button \"Sign in\"\nref=e7 textbox \"Search\"\nintent: log into the account"))),
        ("choice_5", with_none(choice(
            "Pick the topic of this transcript batch",
            [("1", "quarterly budget review"), ("2", "hiring plan"), ("3", "product roadmap"),
             ("4", "customer escalation"), ("5", "office move")],
            "Speaker A: we still need two more backend engineers before Q3. Speaker B: and the "
            "budget for that was approved last week."))),
        ("choice_48", with_none(choice(
            "Which element should be clicked next?",
            [(f"e{i}", f"element {i} {kind}") for i, kind in enumerate(
                ["button", "link", "textbox", "checkbox", "menu", "tab"] * 8)],
            STATE_48))),
        ("bool_plain", {"type": "bool",
                        "statement": "the page text above is sufficient to complete the current step",
                        "state": "Heading: Checkout\nFields: card number, expiry, CVC\n12 interactive elements"}),
        ("score_3", {"type": "score", "instructions": "How urgent is this ticket?",
                     "levels": ["can wait a week", "this week", "right now"],
                     "state": "Production API returns 500 for every request since 09:12 UTC."}),
        ("score_5", {"type": "score", "instructions": "Rate the sentiment",
                     "levels": ["very negative", "negative", "neutral", "positive", "very positive"],
                     "state": "The replacement arrived two days early and works perfectly, thanks!"}),
        ("empty_state", with_none(choice("What should happen next?",
                                          [("stop", "stop recording"), ("go", "keep recording")], ""))),
        ("long_state", with_none(choice("Which warehouse is mentioned most?",
                                         [("w0", "warehouse 0"), ("w3", "warehouse 3")], LONG_STATE))),
        ("non_ascii", with_none(choice(
            "Quelle est la langue du texte ?",
            [("fr", "français"), ("de", "Deutsch"), ("ja", "日本語")],
            "Le café était délicieux — naïve résumé, Ünïcödé ÀÉÎÕÜ, 日本語のテキスト, straße, İstanbul."))),
        ("punct_upper", with_none(choice(
            "Which menu item?",
            [("a", "File > Save As…"), ("b", "EDIT: Undo (Ctrl+Z)"), ("c", "Help / About — v2.0!"),
             ("d", "")],
            "the user pressed CTRL+S then said 'no wait, undo that'."))),
        ("colon_instructions", with_none(choice(
            "Decide: which tool runs next: search or read?",
            [("search", "brief_search"), ("read", "read the page")],
            "Query: latest earnings call transcript for ACME Corp; page already open."))),
        ("bool_no_period", {"type": "bool", "statement": "the statement holds",
                            "state": "ends with a question mark?"}),
        ("score_bang", {"type": "score", "instructions": "rate it",
                        "levels": ["low", "high"], "state": "already ends with a bang!"}),
    ]


def build(processor, hf_tok, raw_tok, q):
    task, text = task_and_text(q)
    schema = {"classifications": [{"task": task, "labels": labels(q), "multi_label": False,
                                   "cls_threshold": 0.5, "true_label": ["N/A"]}]}
    # gliner2/processor.py `_collate_batch` (terminal ".", deepcopy) → `_transform_record`
    # (word split + lower) → `_format_input_with_mapping` (per-piece tokenize, [P]/[L] slots);
    # `_pad_batch` carries each record's `schema_special_positions` as `schema_special_indices`.
    batch = processor._collate_batch([(text, schema)], max_len=None, error_policy="raise")
    ids = batch.input_ids[0].tolist()
    mask = batch.attention_mask[0].tolist()
    assert mask == [1] * len(ids), "single row: no padding expected"
    positions = batch.schema_special_indices[0][0]
    assert ids[positions[0]] == MARKERS["[P]"]
    label_pos = positions[1:]
    assert all(ids[p] == MARKERS["[L]"] for p in label_pos)
    # The Rust port encodes each piece with `Tokenizer::encode(piece, false)`; prove the HF
    # `tokenize()` → `convert_tokens_to_ids` round trip gliner2 uses is the same ids.
    pieces = ["(", "[P]", task, "("]
    for l in labels(q):
        pieces += ["[L]", l]
    pieces += [")", ")", "[SEP_TEXT]"]
    t = text if not text else (text if text.endswith((".", "!", "?")) else text + ".")
    t = t or "."
    pieces += [w for w, _, _ in processor.word_splitter(t, lower=True)]
    direct = [i for p in pieces for i in raw_tok.encode(p, add_special_tokens=False).ids]
    assert direct == ids, ("per-piece encode differs from gliner2", pieces)
    return {"name": None, "question": q, "labels": labels(q), "input_ids": ids,
            "attention_mask": mask, "label_pos": label_pos,
            "decoded": hf_tok.decode(ids, skip_special_tokens=False)}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true", help="diff against the committed JSON")
    args = ap.parse_args()

    d = model_dir()
    from tokenizers import Tokenizer
    from transformers import AutoTokenizer
    from gliner2.processor import SchemaTransformer

    hf_tok = AutoTokenizer.from_pretrained(str(d))
    raw_tok = Tokenizer.from_file(str(d / "tokenizer.json"))
    for tok, want in MARKERS.items():
        assert hf_tok.convert_tokens_to_ids(tok) == want == raw_tok.token_to_id(tok), tok
    processor = SchemaTransformer(tokenizer=hf_tok)
    processor.change_mode(is_training=False)

    out = {"repo": REPO, "revision": REVISION, "gliner2": "2.0.0", "markers": MARKERS, "cases": []}
    for name, q in cases():
        c = build(processor, hf_tok, raw_tok, q)
        c["name"] = name
        out["cases"].append(c)
    text = json.dumps(out, ensure_ascii=False, indent=1) + "\n"

    if args.check:
        have = OUT.read_text() if OUT.exists() else ""
        if have != text:
            import difflib

            sys.stdout.writelines(difflib.unified_diff(
                have.splitlines(True), text.splitlines(True), str(OUT), "regenerated"))
            return 1
        print(f"ok: {len(out['cases'])} cases match {OUT.relative_to(ROOT)}")
        return 0
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(text)
    for c in out["cases"]:
        print(f"{c['name']:>20} len={len(c['input_ids']):4} labels={len(c['label_pos'])}")
    print(f"wrote {OUT.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
