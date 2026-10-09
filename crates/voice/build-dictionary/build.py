#!/usr/bin/env python3
"""Compose crates/voice/data/dictionary.v1.json.gz — the packaged technical dictionary (153 §5.1–5.2).

Run by a maintainer, never at build time; the output is committed. Stdlib only.

    python3 crates/voice/build-dictionary/build.py            # fetch (cached under cache/) and write
    python3 crates/voice/build-dictionary/build.py --offline  # cache only; a missing source is skipped

Every fetch is cached as cache/<name>; a source that fails is skipped and listed in the record's
`skipped`. Output: data/dictionary.v1.json.gz, data/dictionary.json (sha256 record the loader verifies).
Sources and licences: data/THIRD_PARTY.md (hand-written; update it when a source changes).

Packaged term shape (serde names match voice::dictionary::Kind): {"c": canonical, "k": kind, "s": [spoken…], "t": [tags…]}.
Spoken forms that normalise to the canonical (case, separators, digits) are NOT stored — the matcher's norm() finds them.
"""

import ast
import gzip
import hashlib
import io
import json
import re
import sys
import tarfile
import urllib.request
import zipfile
from datetime import datetime, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent
CACHE = HERE / "cache"
DATA = HERE.parent / "data"
OFFLINE = "--offline" in sys.argv
UA = {"User-Agent": "theuth-build-dictionary/1 (+https://github.com/theuth)"}

SKIPPED = []  # (source, reason)
COUNTS = {}  # source -> terms added


def fetch(name, url, binary=False):
    """Cached GET; raises on failure (callers wrap with `source`)."""
    CACHE.mkdir(exist_ok=True)
    p = CACHE / name
    if p.exists():
        return p.read_bytes() if binary else p.read_text()
    if OFFLINE:
        raise RuntimeError("offline and not cached")
    req = urllib.request.Request(url, headers=UA)
    with urllib.request.urlopen(req, timeout=120) as r:
        body = r.read()
        link = r.headers.get("link", "")
    p.write_bytes(body)
    if name.startswith("hf-models-"):  # keep the HF cursor next to the page
        (CACHE / (name + ".link")).write_text(link)
    return body if binary else body.decode()


class source:
    """`with source("name"): …` — a failure skips the source and records why."""

    def __init__(self, name):
        self.name = name
        self.before = len(TERMS)

    def __enter__(self):
        return self

    def __exit__(self, et, ev, tb):
        if et is None:
            COUNTS[self.name] = len(TERMS) - self.before
            print(f"  {self.name}: {COUNTS[self.name]} terms")
            return False
        del TERMS[self.before :]
        SKIPPED.append((self.name, f"{et.__name__}: {ev}"))
        print(f"  {self.name}: SKIPPED ({et.__name__}: {ev})", file=sys.stderr)
        return True


TERMS = []  # [(canonical, kind, spoken[], tags[])] in catalog order


def norm(s):
    return re.sub(r"[^a-z0-9]+", "", s.lower())


def add(canonical, kind="term", spoken=(), tags=()):
    canonical = canonical.strip()
    if not canonical or len(canonical) > 64:
        return
    n = norm(canonical)
    if kind == "term" and len(n) < 2 and "number" not in tags:
        return
    if kind in ("term", "path") and canonical == canonical.lower() and "keep_case" not in tags and any(c.isalpha() for c in canonical):
        tags.append("keep_case")
    TERMS.append((canonical, kind, list(dict.fromkeys(spoken)), tags))


# ---------------------------------------------------------------- talonhub/community (MIT)
def talon():
    raw = "https://raw.githubusercontent.com/talonhub/community/main/"
    with source("talon-symbols"):
        src = fetch("symbols.py", raw + "core/keys/symbols.py")
        ns = {}

        class Symbol:
            def __init__(self, ch, both=None, cmd=None):
                self.ch, self.forms = ch, ([both] if isinstance(both, str) else both or []) + ([cmd] if isinstance(cmd, str) else cmd or [])

        ns["Symbol"] = Symbol
        for var in ("currency_symbols", "old_symbols"):
            m = re.search(rf"^{var} = \[\n(.*?)^\]", src, re.S | re.M)
            exec(f"{var} = [\n{m.group(1)}]", ns)
        for s in ns["old_symbols"] + ns["currency_symbols"]:
            add(s.ch, "symbol", s.forms, ["talon"])
    with source("talon-formatters"):
        src = fetch("code_formatter.talon-list", raw + "core/formatters/code_formatter.talon-list")
        plain = {  # the plain dictation forms of §4.2 stage 4, beside talon's terse names
            "PRIVATE_CAMEL_CASE": ["camel case"],
            "PUBLIC_CAMEL_CASE": ["pascal case"],
            "SNAKE_CASE": ["snake case"],
            "DASH_SEPARATED": ["kebab case"],
            "ALL_CAPS,SNAKE_CASE": ["constant case"],
            "ALL_CAPS": ["all caps"],
            "DOT_SEPARATED": ["dotted"],
        }
        for line in src.splitlines():
            if ":" in line and not line.startswith(("list:", "-")):
                spoken, code = (x.strip() for x in line.split(":", 1))
                add(code, "formatter", [spoken] + plain.get(code, []), ["talon", "keep_case"])
    with source("talon-abbreviations"):
        src = fetch("abbreviate.py", raw + "core/abbreviate/abbreviate.py")
        m = re.search(r"^abbreviations = (\{.*?^\})", src, re.S | re.M)
        for spoken, abbr in ast.literal_eval(m.group(1)).items():
            add(abbr, "abbrev", [spoken], ["talon"])
    with source("talon-numbers"):
        src = fetch("numbers.py", raw + "core/numbers/numbers.py")
        ns = {}
        for var in ("digit_list", "teens", "tens", "scales"):
            m = re.search(rf"^{var} = (\[.*?\])", src, re.S | re.M)
            ns[var] = ast.literal_eval(m.group(1))
        for i, w in enumerate(ns["digit_list"]):
            add(str(i), "term", [w] + (["oh"] if i == 0 else []), ["talon", "number"])
        for i, w in enumerate(ns["teens"]):
            add(str(10 + i), "term", [w], ["talon", "number"])
        for i, w in enumerate(ns["tens"]):
            add(str(10 * (i + 2)), "term", [w], ["talon", "number"])
        for i, w in enumerate(ns["scales"]):
            add(str(100 if i == 0 else 10 ** (3 * i)), "term", [w], ["talon", "number"])
    with source("talon-extensions"):
        src = fetch("file_extension.py", raw + "core/file_extension/file_extension.py")
        m = re.search(r"^_file_extensions_defaults = (\{.*?^\})", src, re.S | re.M)
        by_ext = {}
        for spoken, ext in ast.literal_eval(m.group(1)).items():
            by_ext.setdefault(ext, []).append(re.sub(r"^dot ", "", spoken))
        for ext, forms in by_ext.items():
            add(ext, "path", forms, ["talon", "ext"])
    with source("talon-tlds"):
        src = fetch("top_level_domains.talon-list", raw + "core/top_level_domain/top_level_domains.talon-list")
        for line in src.splitlines():
            if ":" in line and not line.startswith(("list:", "-")):
                spoken, tld = (x.strip() for x in line.split(":", 1))
                add(tld, "path", [re.sub(r"^dot ", "", spoken)], ["talon", "tld"])


# ---------------------------------------------------------------- commands and tools
def homebrew():
    with source("homebrew-formula"):
        for f in json.loads(fetch("formula.json", "https://formulae.brew.sh/api/formula.json")):
            # No `aliases` / `oldnames`: in the replacement direction ("golang" → go) they rewrite prose.
            add(f["name"], "term", [], ["brew", "terminal"])
    with source("homebrew-cask"):
        for c in json.loads(fetch("cask.json", "https://formulae.brew.sh/api/cask.json")):
            names = [n for n in c.get("name", []) if n]
            if names:
                add(names[0], "term", names[1:], ["brew", "app"])


def tldr():
    with source("tldr-pages"):
        z = zipfile.ZipFile(io.BytesIO(fetch("tldr-pages.en.zip", "https://github.com/tldr-pages/tldr/releases/latest/download/tldr-pages.en.zip", binary=True)))
        for n in sorted(z.namelist()):
            m = re.fullmatch(r"(?:pages/)?common/([^/]+)\.md", n)
            if m:
                add(m.group(1), "term", [], ["tldr", "terminal"])


def ohmyzsh():
    with source("oh-my-zsh-plugins"):
        for d in json.loads(fetch("omz-plugins.json", "https://api.github.com/repos/ohmyzsh/ohmyzsh/contents/plugins")):
            if d["type"] == "dir":
                add(d["name"], "term", [], ["omz", "terminal"])


def linguist():
    with source("linguist-languages"):
        src = fetch("languages.yml", "https://raw.githubusercontent.com/github-linguist/linguist/main/lib/linguist/languages.yml")
        # Names only; the `aliases` ("man" → Roff, "python3" → Python) rewrite prose in the replacement direction.
        for line in src.splitlines():
            if re.match(r"^\S", line) and line.rstrip().endswith(":") and not line.startswith("#"):
                add(line.rstrip()[:-1].strip('"'), "term", [], ["linguist", "language"])


def pypi():
    with source("top-pypi-packages"):
        for row in json.loads(fetch("top-pypi-packages.min.json", "https://hugovk.dev/top-pypi-packages/top-pypi-packages.min.json"))["rows"]:
            add(row["project"], "term", [], ["pypi"])


# ---------------------------------------------------------------- cloud
def botocore():
    with source("botocore-services"):
        meta = json.loads(fetch("botocore-pypi.json", "https://pypi.org/pypi/botocore/json"))
        sdist = next(u for u in meta["urls"] if u["packagetype"] == "sdist")
        tar = tarfile.open(fileobj=io.BytesIO(fetch("botocore.tar.gz", sdist["url"], binary=True)))
        seen = set()
        for m in tar.getmembers():
            mm = re.search(r"botocore/data/([^/]+)/[^/]+/service-2\.json$", m.name)
            if mm and mm.group(1) not in seen:
                seen.add(mm.group(1))
                md = json.load(tar.extractfile(m))["metadata"]
                add(mm.group(1), "term", [], ["aws", "terminal"])
                for name in (md.get("serviceFullName", ""), md.get("serviceAbbreviation", "")):
                    if name and len(name.split()) <= 5:
                        add(name, "term", [], ["aws"])


def kubernetes():
    with source("kubernetes-kinds"):
        spec = json.loads(fetch("k8s-swagger.json", "https://raw.githubusercontent.com/kubernetes/kubernetes/master/api/openapi-spec/swagger.json"))
        kinds = {g["kind"] for d in spec["definitions"].values() for g in d.get("x-kubernetes-group-version-kind", [])}
        for k in sorted(kinds):
            add(k, "term", [], ["k8s"])


# ---------------------------------------------------------------- AI vocabulary
def clean(name):
    """`Grok 4.6 (Vertex AI (OpenAI-compatible))` → `Grok 4.6`."""
    return re.sub(r"\s*\(.*$", "", name).strip()


def models_dev():
    with source("models.dev"):
        api = json.loads(fetch("models.dev.json", "https://models.dev/api.json"))
        for pid, p in api.items():
            add(clean(p.get("name") or pid), "term", [pid.replace("-", " ")], ["ai", "provider"])
            for mid, m in p.get("models", {}).items():
                sid = mid.rsplit("/", 1)[-1]
                # Ids with a digit or a hyphen only: a bare word id ("small" → Auto Small) rewrites prose.
                spoken = [sid.replace("-", " ")] if re.fullmatch(r"[A-Za-z0-9.-]{1,40}", sid) and re.search(r"[0-9-]", sid) else []
                add(clean(m.get("name") or mid), "term", spoken, ["ai", "model"])


def huggingface():
    with source("huggingface-top-models"):
        url = "https://huggingface.co/api/models?sort=downloads&direction=-1&limit=1000"
        for page in range(2):
            name = f"hf-models-{page}.json"
            rows = json.loads(fetch(name, url))
            for r in rows:
                mid = r.get("id") or r.get("modelId")
                if not mid:
                    continue
                org, _, model = mid.rpartition("/")
                add(model, "term", [], ["ai", "model", "hf"])
                if org:
                    add(org, "term", [], ["ai", "org", "hf"])
            link = (CACHE / (name + ".link")).read_text() if (CACHE / (name + ".link")).exists() else ""
            m = re.search(r"<([^>]+)>;\s*rel=\"next\"", link)
            if not m:
                break
            url = m.group(1)


CURATED = """
# Only names the model would plausibly hear; English words that happen to be product names (Arc, Slack,
# Windows, React…) are left out: a Title-case hit would capitalise them in prose.
Claude Code|claude code
Codex
Cursor
Windsurf
OpenCode|open code
Gemini CLI|gemini c l i
omp|keep_case
Aider
Cline
Copilot|co pilot
theuth|keep_case
Anthropic
OpenAI|open a i
Gemini
GPT|g p t
Qwen|quen
Llama
Mistral
DeepSeek|deep seek
Parakeet
Nemotron
Whisper
ONNX|onyx
Tauri
Rust
cargo|keep_case
crates.io|crates dot i o
Swift
Xcode|x code
Homebrew|home brew
kubectl|keep_case|cube control|cube c t l
k8s|kates
Kubernetes|cooper netties
Terraform|terra form
Docker
Podman|pod man
Nix
Bun
Vite|veet
Vitest|vee test
tsc|keep_case
pnpm|keep_case
npm|keep_case
uv|keep_case
Poetry
SQLite|sequel lite|s q lite
Postgres|post gres
PostgreSQL|postgres q l|post gres sequel
Redis
Kafka
gRPC|g r p c|keep_case
GraphQL|graph q l
WebGPU|web g p u
Metal
CUDA|cooda
GitHub|git hub|get hub
GitLab|git lab
Git
JSON|jason
YAML|yamel
TOML
TypeScript|type script
JavaScript|java script
Python
Node.js|node j s|node
Deno
Next.js|next j s
Nuxt
Vue
Zod
tokio|keep_case
serde|keep_case|sir dee
clippy|keep_case
rustfmt|keep_case|rust format
rustc|keep_case
ripgrep|keep_case|rip grep
fzf|keep_case
tmux|keep_case|t mux
Neovim|neo vim
Emacs|e max
VS Code|vs code|visual studio code
JetBrains|jet brains
Ghostty|ghosty
iTerm|i term
WezTerm|wez term
Alacritty
Linux
macOS|mac o s|mac os
iOS|i o s
Android
WebSocket|web socket
HTTP|h t t p
HTTPS|h t t p s
OAuth|o auth
JWT|j w t
SSH|s s h
TLS|t l s
DNS|d n s
CLI|c l i
API|a p i
SDK|s d k
RPC|r p c
MCP|m c p
LLM|l l m
RAG|keep_case
ASR|a s r
TTS|t t s
STT|s t t
VAD|v a d
ONNX Runtime|onyx runtime
PyTorch|pie torch
TensorFlow|tensor flow
JAX
NumPy|num pie
pandas|keep_case
Jupyter|jupiter
Hugging Face|hugging face
Ollama|o llama
vLLM|v l l m
llama.cpp|llama c p p|llama cpp
LangChain|lang chain
LlamaIndex|llama index
OpenRouter|open router
Groq|grok
Cerebras
Together AI|together a i
Fireworks AI|fireworks a i
xAI|x a i
Grok
Vertex AI|vertex a i
AWS|a w s
GCP|g c p
Cloudflare|cloud flare
Vercel
Netlify
Supabase|supa base
Firebase|fire base
Nginx|engine x
systemd|keep_case|system d
Ansible
Pulumi
Prometheus
Grafana
Datadog|data dog
OpenTelemetry|open telemetry
Jira
Figma
Raycast|ray cast
Firefox
"""


def curated():
    with source("curated-harnesses"):
        for line in CURATED.strip().splitlines():
            if line.startswith("#"):
                continue
            parts = [p.strip() for p in line.split("|")]
            tags = ["curated"] + (["keep_case"] if "keep_case" in parts[1:] else [])
            add(parts[0], "term", [p for p in parts[1:] if p != "keep_case"], tags)


# ---------------------------------------------------------------- compose
def main():
    print("building dictionary.v1.json.gz")
    curated()  # first: its casing wins the dedup
    talon()
    models_dev()
    huggingface()
    linguist()
    kubernetes()
    botocore()
    homebrew()
    tldr()
    ohmyzsh()
    pypi()

    merged = {}  # key -> [canonical, kind, spoken, tags]
    for canonical, kind, spoken, tags in TERMS:
        key = (kind, canonical if kind == "symbol" else norm(canonical))
        if key in merged:
            e = merged[key]
            e[2] = list(dict.fromkeys(e[2] + spoken))
            e[3] = list(dict.fromkeys(e[3] + tags))
            if "keep_case" in e[3] and e[0] != e[0].lower():
                e[3].remove("keep_case")
        else:
            merged[key] = [canonical, kind, spoken, tags]
    terms = [{"c": c, "k": k, "s": s, "t": t} for c, k, s, t in merged.values()]
    pairs = sum(1 for t in terms if t["k"] in ("symbol", "formatter", "abbrev") or "number" in t["t"])
    built_at = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    body = json.dumps({"version": 1, "built_at": built_at, "terms": terms}, ensure_ascii=False, separators=(",", ":")).encode()
    gz = io.BytesIO()
    with gzip.GzipFile(fileobj=gz, mode="wb", mtime=0, compresslevel=9) as g:
        g.write(body)
    data = gz.getvalue()
    DATA.mkdir(exist_ok=True)
    (DATA / "dictionary.v1.json.gz").write_bytes(data)
    record = {
        "version": 1,
        "file": "dictionary.v1.json.gz",
        "sha256": hashlib.sha256(data).hexdigest(),
        "bytes": len(data),
        "terms": len(terms),
        "pairs": pairs,
        "built_at": built_at,
        "sources": [{"name": k, "terms": v} for k, v in COUNTS.items()],
        "skipped": [{"name": k, "reason": v} for k, v in SKIPPED],
    }
    (DATA / "dictionary.json").write_text(json.dumps(record, indent=2) + "\n")
    print(f"{len(terms)} terms ({pairs} spoken-form pairs), {len(data)} bytes gz, raw {len(body)}; skipped {len(SKIPPED)}")
    if len(data) > 1_572_864:
        print("WARNING: gz > 1.5 MiB — include_bytes! budget exceeded", file=sys.stderr)


if __name__ == "__main__":
    main()
