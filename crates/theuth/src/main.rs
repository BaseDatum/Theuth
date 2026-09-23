//! `theuth`: build the runtime, start the plugin list, wait for shutdown.
//! Also the Python runtime worker: `t subprocess` (design doc 08 §9, [`worker`]).

mod bench_models;
mod link_cli;
mod secret_cli;
mod service;
mod worker;

use std::path::{Path, PathBuf};

use clap::Parser;
use figment::Figment;
use figment::providers::{Env, Format, Yaml};
use runtime::{Plugin, Runtime, SessionId};

/// The worker's memory limits are enforced by the allocator, so the process
/// that runs `t subprocess` must own it. Unarmed (every other mode) it is
/// `System` plus one live-byte counter.
#[global_allocator]
static ALLOC: monty_alloc::LimitedAllocator = monty_alloc::LimitedAllocator;

#[derive(Parser, Debug)]
#[command(name = "t", version, about = "what the harness")]
struct Cli {
    /// Data directory (config, secrets vault, mcp/, sqlite, logs, token, skills).
    #[arg(long, env = "THEUTH_DATA_DIR")]
    data_dir: Option<PathBuf>,
    /// Config file; defaults to `<data_dir>/config.yaml`.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Start the JSON-RPC server plugin.
    #[arg(long)]
    serve: bool,
    /// Do not start the terminal UI (implies --serve).
    #[arg(long)]
    headless: bool,
    /// Open an existing session in the terminal UI.
    #[arg(long)]
    resume: Option<SessionId>,
    /// Print mode: send this prompt (or stdin when omitted), stream the reply
    /// to stdout, exit when the turn finishes. No full-screen UI.
    #[arg(short, long, num_args = 0..=1, default_missing_value = "")]
    prompt: Option<String>,
    /// Speak the Agent Client Protocol on stdio (for editors); implies no TUI and no RPC server.
    #[arg(long, conflicts_with_all = ["headless", "serve", "prompt", "link_signal"])]
    acp: bool,
    /// Pair this machine as a Signal linked device (one-shot: prints a QR,
    /// waits for the scan, writes `signal.owner`/`device_name`, exits).
    #[arg(long)]
    link_signal: bool,
    /// Owner E.164 (`+15551234567`) for --link-signal; prompted on a TTY when absent.
    #[arg(long, requires = "link_signal")]
    owner: Option<String>,
    /// Device name shown in the phone's Linked devices list (default: config `signal.device_name`).
    #[arg(long, requires = "link_signal")]
    device_name: Option<String>,
    /// Print the Slack app manifest (paste into api.slack.com/apps → Create from manifest) and exit.
    #[arg(long, conflicts_with_all = ["headless", "serve", "acp", "link_signal"])]
    slack_manifest: bool,
    /// Interactive Slack setup: create the app from the manifest (or update
    /// `--app`), verify the bot and app-level tokens, set the owner, write the
    /// tokens into the secrets vault and the owner into config.yaml, exit.
    #[arg(long, conflicts_with_all = ["headless", "serve", "acp", "link_signal", "slack_manifest", "prompt"])]
    slack_setup: bool,
    /// Existing Slack app id (`A…`) that --slack-setup updates to the manifest instead of creating one.
    #[arg(long, requires = "slack_setup")]
    app: Option<String>,
    /// Pair a phone with the running daemon (one-shot: prints a QR and the
    /// key, waits for the scan, exits). Needs `t --headless` on this box.
    #[arg(long, conflicts_with_all = ["headless", "serve", "acp", "prompt", "link_signal", "slack_manifest", "slack_setup"])]
    link: bool,
    /// Install this harness as a background service (launchd LaunchAgent or
    /// systemd user unit) that runs `t --headless` through the login shell,
    /// set `link.enabled: true`, start it, then pair it to your main harness
    /// as a node (design doc 32 §5.1).
    #[arg(long, conflicts_with_all = ["headless", "serve", "acp", "prompt", "link_signal", "slack_manifest", "slack_setup", "link", "resume", "uninstall_service"])]
    install_service: bool,
    /// Stop and remove the service unit written by --install-service. The data dir is untouched.
    #[arg(long, conflicts_with_all = ["headless", "serve", "acp", "prompt", "link_signal", "slack_manifest", "slack_setup", "link", "resume"])]
    uninstall_service: bool,
    /// Relay WebSocket URL for --install-service (`link.relay`); default: the configured relay.
    #[arg(long, requires = "install_service")]
    relay: Option<String>,
    /// Display name for --install-service (`link.name`); default: hostname.
    #[arg(long, requires = "install_service")]
    name: Option<String>,
    /// --install-service: skip the pairing step after the service starts.
    #[arg(long, requires = "install_service")]
    no_pair: bool,
    /// Store a secret: value read from the terminal without echo (asked twice),
    /// or the first line of stdin when piped. Exit 0 with `stored NAME vN`.
    #[arg(long, value_name = "NAME", conflicts_with_all = ["headless", "serve", "acp", "prompt", "resume", "link_signal", "slack_manifest", "slack_setup", "link", "install_service", "uninstall_service", "secret_rm", "secret_list"])]
    secret_set: Option<String>,
    /// Note shown in `secret_list` next to the name stored by --secret-set.
    #[arg(long, requires = "secret_set")]
    secret_note: Option<String>,
    /// Delete a secret and every version of it.
    #[arg(long, value_name = "NAME", conflicts_with_all = ["headless", "serve", "acp", "prompt", "resume", "link_signal", "slack_manifest", "slack_setup", "link", "install_service", "uninstall_service", "secret_list"])]
    secret_rm: Option<String>,
    /// List secrets: names, versions, notes, dates. Never values.
    #[arg(long, conflicts_with_all = ["headless", "serve", "acp", "prompt", "resume", "link_signal", "slack_manifest", "slack_setup", "link", "install_service", "uninstall_service"])]
    secret_list: bool,
    /// Check the release manifest once, verify, replace this binary on disk
    /// (or `brew upgrade theuth` for a Homebrew install), print one line, exit (39 §3.2).
    #[arg(long, conflicts_with_all = ["headless", "serve", "acp", "prompt", "resume", "link_signal", "slack_manifest", "slack_setup", "link", "install_service", "uninstall_service", "secret_set", "secret_rm", "secret_list"])]
    update: bool,
    /// Print the app library compatibility hash this binary was built with and
    /// exit (46 §2.1): the updater checks a staged `theuth-app-parser-<hash>` helper
    /// against it. Before config, logging or any plugin.
    #[arg(long, conflicts_with_all = ["headless", "serve", "acp", "prompt", "resume", "link_signal", "slack_manifest", "slack_setup", "link", "install_service", "uninstall_service", "secret_set", "secret_rm", "secret_list", "update"])]
    app_parser_hash: bool,
    /// 57 §11.4: fold the captured training rows of a decision family prefix (`meeting`) into
    /// the phrasing-bench fixture shape (or laya's typed-decisions rows with --format laya),
    /// print (or --out) and exit. Reads <data_dir>/decisions/training/<prefix>/*.jsonl.
    #[arg(long, value_name = "PREFIX", conflicts_with_all = ["headless", "serve", "acp", "prompt", "resume", "link_signal", "slack_manifest", "slack_setup", "link", "install_service", "uninstall_service", "secret_set", "secret_rm", "secret_list", "update"])]
    decision_export: Option<String>,
    /// --decision-export: only day files from this date on (YYYY-MM-DD).
    #[arg(long, requires = "decision_export", value_name = "YYYY-MM-DD")]
    since: Option<chrono::NaiveDate>,
    /// --decision-export: write here instead of stdout.
    #[arg(long, requires = "decision_export", value_name = "FILE")]
    out: Option<PathBuf>,
    /// --decision-export: `fixture` (default) | `laya`.
    #[arg(long, requires = "decision_export", default_value = "fixture")]
    format: String,
    /// 58 step 12: load ONNX Runtime and print the execution provider sessions get, run the
    /// STT encoder once on 5 s of silence, open the decision checkpoint and run one question,
    /// each with its ms and device; exit 2 naming what is missing. `$ORT_DYLIB_PATH` honoured.
    #[arg(long, conflicts_with_all = ["headless", "serve", "acp", "prompt", "resume", "link_signal", "slack_manifest", "slack_setup", "link", "install_service", "uninstall_service", "secret_set", "secret_rm", "secret_list", "update", "decision_export"])]
    bench_models: bool,
    /// --bench-models: override `models.accelerator` (`auto` | `cpu` | `webgpu` | `directml` | `cuda`).
    #[arg(long, requires = "bench_models", value_name = "ACCEL")]
    accelerator: Option<models::accelerator::Accelerator>,
}

const DEFAULT_CONFIG: &str = "# theuth configuration. Every plugin reads its own section.
# Env overrides: THEUTH_<SECTION>__<KEY>, e.g. THEUTH_RPC__LISTEN=127.0.0.1:7799

agent:
  model: anthropic/claude-sonnet-4-5
  communication: false       # model-to-model communication (design doc 50): spawn_subagent, discover_peers, send_message.
                             # Off is a ceiling no session can override; `/communication on always` writes true here.
                             # `/communication off` in a session is memory-only and ends with the process.
  # title_model: anthropic/claude-haiku-4-5   # names sessions in the history list (07 §15); default: the session's model
  # vision_model: anthropic/claude-haiku-4-5  # answers the main model's questions about images it cannot see (52 §3);
                             # unset: no fallback. Must be an exact catalog `provider/model` that accepts images;
                             # images then go to that provider, with extra calls. /vision or Settings → Agent loop
  # escalate_model: anthropic/claude-haiku-4-5  # tier 2 of the local decision oracle (57 §0.9): one cheap no-tools call answers a typed question the oracle abstained on;
                             # unset: inherits title_model, then the session's model. Must be a catalog `provider/model`. /escalate or Settings → Agent loop
  # stream_idle_secs: 120     # seconds without a stream event before a model call is abandoned (36 §2.3); Settings → Agent loop
  # max_salvages: 2           # interrupted-reply recoveries per turn (stalled stream or output cap) before the turn errors (36 §2.4); 0 = off
  # aws_profile: work          # Bedrock inference for every session; /aws or Settings → Connections
  # subagents:
  #   model: null              # preferred subagent model when spawn_subagent omits `model`, named in the system prompt; default: agent.model. /subagent or Settings → Agent loop
  #   max_depth: 1
  #   max_concurrent: 8
  #   max_runtime_secs: 600
  #   reply_timeout_secs: 120
  # knowledge:                # loop policy (design doc 10 §5); the store itself is `knowledge:` below
  #   hint_limit: 5            # lines per <knowledge> note
  #   hint_rounds: 4           # mid-turn hint cadence in rounds
  #   harvest_every: 3         # tool-using root turns per harvest; 0 disables harvest
  #   harvest_max: 8           # documents accepted per harvest

# knowledge:                 # design doc 10; the bundle lives in <data_dir>/knowledge/
#   enabled: true            # false: no store, no knowledge_* tools, no hints, no harvest

# documents:                 # design doc 59; Yjs documents under <data_dir>/documents/, the BlockNote editor
#   enabled: true            # false: no store, no helper, no doc_* methods; concepts open as raw text
#   export_debounce_ms: 400  # settle time before the derived .md is rewritten (hard cap 3 s)
#   export_quiet_secs: 60    # no typing for this long before the owner records the edit (knowledge commit, artifact version);
#                            # flushed at once when the document closes, its last reader leaves, or theuth stops
#   max_doc_bytes: 8388608   # an update that would grow a document past this is refused (DocTooLarge)
#   snapshots_keep: 30       # automatic (daily) snapshots kept per document; labelled ones are never pruned
#   agent_color: '#7c3aed'  # the agent's presence and typewriter reveal colour in every editor
#   max_members: 64          # members per shared document; the 65th join is refused
#   sharing: on              # off: no new share links, members still connect; disabled: members refused too
#   idle_unload_secs: 600    # unload an unsubscribed document after this idle time (log compacted into snapshot.yjs)
#   max_open_docs: 256       # documents held in memory at most; the longest idle is unloaded past it
#                            # guest handshakes are rated under `link.guest_handshakes_per_minute` (10)

# surface:                   # Mermaid / json-render blocks in replies (design doc 30)
#   enabled: true            # false: no prompt block, no rendering; fences stay code blocks
#   correct: true            # one <surface_error> fix-up round when a json-render block is invalid

# web:                       # search_web / read_url (design doc 16)
#   enabled: true
#   engines: [google, bing, brave, qwant, duckduckgo, wikipedia, arxiv, github]   # keyless built-ins
#   fetch_max_chars: 4000
#   read_top: 5              # search_web reads this many top hits
#   read_chars: 2500         # passage budget per read page
#   read_timeout_ms: 25000        # a client-rendered page takes obscura ~20s

# scrape:                    # scrape_url / crawl / map_site (design doc 38)
#   concurrency: 5
#   max_pages: 50
#   max_depth: 3
#   same_domain: true
#   page_timeout_ms: 8000
#   run_timeout_ms: 60000
#   robots: true
#   delay_ms: 100            # per-host gap between requests; a larger robots Crawl-delay wins

# codeindex:                 # design doc 60; indexes live in <data_dir>/codeindex/<slug>-<hash>.sqlite
#   enabled: true            # false: no plugin, no code_* functions, nothing on disk
#   sync_limit: 64           # changed files re-indexed synchronously before answering; more → answer stale, finish in background
#   max_file_bytes: 1048576  # larger files are listed, not parsed
#   cochange_commits: 300    # first-parent commits walked for co-change; 0 disables the git walk
#   languages: [rust, typescript, javascript, python, go]   # subset to parse; others are files only

# search_providers:          # optional external engines registered with `web` (design doc 16 §3); the
#                            # built-in keyless engines above need none of these
#   firecrawl:               # inert until the `firecrawl` secret exists (`t --secret-set firecrawl`)
#     base_url: https://api.firecrawl.dev
#   searxng:                 # a SearXNG instance you run
#     base_url: http://127.0.0.1:8080
#   openserp:
#     base_url: http://127.0.0.1:7000
#     engines: [google, bing]
#   brave: {}                # Brave Search API (provider `brave_api`); needs the `brave` secret

# browser:                   # embedded headless browser, browser_* tools (design doc 23)
#   enabled: true
#   allow_private_network: false   # let the browser reach localhost / private IPs
#   import_cookies: false    # adds browser_list_profiles + browser_import_cookies:
#                            # the agent loads the user's real browser cookies and acts as them (§7)

# computer:                  # see and drive this desktop, computer_* tools (design doc 27)
#   enabled: true            # the OS permission prompts are the consent; actions still ask
#   max_edge: 1568           # screenshot long edge after logical scaling (~1280 for Anthropic click accuracy)

# link:                      # drive this harness from a phone through the relay (design doc 31). `t --link` / `/link` pair; `t --install-service` runs it as a node (32).
#   enabled: false           # `/link on`, Settings › Link or hand-edit; the uplink dials out, nothing listens
#   relay: wss://relay.theuth.io   # self-host: your `theuth-relay` URL
#   name: \"\"                 # display name in the QR and on devices; \"\" → hostname
#   max_devices: 16
#   guest_handshakes_per_minute: 10  # share-link guests (design doc 59 §8.3) admitted per minute; over budget → CLOSE 3

# nodes:                     # harnesses on other machines this one can run sessions on (design doc 32). `/nodes add <key>` pairs; the node runs `t --install-service`.
#   enabled: true            # false: no dials, nodes invisible to the model and the UI; nodes.yaml kept

# schedule:                  # scheduled runs from <data_dir>/schedules/*.md files (design doc 34); `/schedules` lists them
#   enabled: true            # false: no watcher, no actor, no tool, no prompt block
#   run_timeout_secs: 1800   # a run still going after this is cancelled and reported as `timeout`

# update:                    # self-update (design doc 39): check the release bucket, verify the signature, replace this
#                            # binary on disk; takes effect at the next start. Nothing is ever asked. `t --update` forces a check.
#   enabled: true
#   url: https://s3.rwx.dev/theuth/releases/latest.json

# artifact:                  # `show` tool: files the agent makes for the user, as cards in the chat (design doc 35); `/artifacts` lists them
#   enabled: true            # false: no `show` tool, no prompt block, no /artifacts; blobs and index kept
#   fonts: []                # extra directories searched for PDF fonts, besides system fonts and the embedded set

# app:                       # `app` tool: interactive OpenUI screens, rendered on desktop/web (design doc 46)
#   enabled: true            # false: no tool, no prompt, no parser helper, no app RPC; existing apps stay readable as source

# voice:                     # read replies aloud (design doc 43): Pocket TTS in-process, audio streamed to the frontend;
#                            # voice notes in, spoken replies out on Signal/Slack (design doc 45); live dictation (design doc 54)
#   enabled: true            # false: no actor, `speak` answers \"not running\", the desktop shows no button or mic
#   voice: peter_yearsley    # one of the checkpoint's voices; `voice_status` lists them
#   model_dir: ~             # a local Pocket TTS checkpoint directory; default <data_dir>/models/pocket-tts
#   stt_model_dir: ~         # Parakeet TDT ONNX checkpoint (encoder-model.onnx, decoder_joint-model.onnx, vocab.txt); default <data_dir>/models/parakeet-tdt.
#                            # Every ONNX model (STT, dictation, diarisation, decision) shares one ONNX Runtime: the .app bundles it; otherwise
#                            # <data_dir>/models/onnxruntime/libonnxruntime.{dylib,so} (fetched by the desktop app, or `$ORT_DYLIB_PATH`, or Homebrew)
#   stt_idle_secs: 600       # unload the speech-to-text model after this long unused; 0 keeps it resident
#   dictation_model_dir: ~   # nemotron-speech-streaming-en ONNX dir (encoder.onnx, decoder_joint.onnx, tokenizer.model); default <data_dir>/models/nemotron-streaming-en
#   dictation_max_secs: 300  # a running dictation stops itself here (design doc 54)

# meeting:                   # record a meeting: the meeting app's audio + the mic as Opus tracks, live briefs from the notch (design doc 53); macOS 15
#   enabled: true            # false: no actor, `meeting_*` answers \"meetings are disabled\"
#   max_minutes: 180         # hard stop; warning (NearMaxLength) at max_minutes - 10
#   context_tokens: 24000    # per-session compaction trigger for the meeting session
#   batch_secs: 25           # transcript batch cadence
#   batch_utterances: 6      # transcript batch size
#   live_model_dir: ~        # Nemotron streaming dir; default <data_dir>/models/nemotron-streaming-en (the dictation checkpoint)
#   topic_ttl_secs: 600      # how long a topic button lives unmentioned
#   max_topics: 6            # most topic buttons at once
#   models_dir: ~            # phase 4: pyannote models; default <data_dir>/models/pyannote
#   live_tools: [meeting_topic, meeting_brief, brief_search, brief_put, knowledge_search, knowledge_get, meeting_read, meeting_search, search_web, read_url]  # 06: the only tools the meeting session sees while recording, all called directly
#   live_prompts: [meeting, knowledge, web]  # 06: the only prompt blocks it sees while recording

# models:                    # where the local models run (design doc 58 steps 12–13). Check with `t --bench-models [--accelerator X]`.
#   accelerator: auto        # auto | cpu | webgpu | directml | cuda. auto = Metal on an Apple-silicon Mac (candle); for the ONNX sessions
#                            # (STT, dictation, diarisation) the WebGPU plugin EP — Dawn on D3D12 / Vulkan / Metal, any GPU — when
#                            # <data_dir>/models/onnxruntime/plugins/{onnxruntime_providers_webgpu.dll | lib….so | lib….dylib} exists
#                            # (Windows: dxcompiler.dll + dxil.dll beside it); CUDA for the decision model when this binary was built
#                            # with `decision/cuda`; else CPU. auto takes WebGPU only on a discrete hardware GPU (NVIDIA/AMD); an
#                            # integrated one (Intel, Apple, APU) or a software adapter (SwiftShader, llvmpipe, WARP — a GPU-less box)
#                            # logs one WARN and stays on the CPU. webgpu forces the plugin on whatever adapter exists, discrete first
#                            # (WARN + CPU when the plugin is missing; WARN + software adapter when that is all there is); directml is
#                            # explicit-only (Microsoft's DirectML ORT build ended at 1.24.4); cuda only affects candle. A GPU that
#                            # fails to initialise logs one WARN and that engine runs on the CPU.
#   webgpu_adapter: ~        # substring of the WebGPU adapter's vendor/description to use when several GPUs answer (e.g. NVIDIA, Intel)

# decision:                  # local typed-probability oracle in front of the reasoning model (design doc 57): laya in candle
#                            # answers typed questions (choice / bool / score) with a probability per option; a per-family gate
#                            # accepts or escalates to today's model turn. Every family starts in shadow (runs, logs, changes nothing).
#   enabled: true            # false: the actor answers every ask with escalate(disabled); nothing loads
#   device: auto             # auto (Metal f16 on an Apple-silicon Mac, else models.accelerator: cuda f32 or CPU f32) | cpu | metal | cuda (warn + CPU when unavailable)
#   model_dir: ~             # the `default` checkpoint dir; default <data_dir>/models/decision: model.safetensors, encoder/config.json,
#                            # tokenizer/tokenizer.json, rl_agent_config.json — the Hugging Face convaiinnovations/laya layout.
#                            # The desktop app fetches it from the release bucket at start like the speech models (54 §9,
#                            # scripts/release-models.sh); the daemon re-checks on every ask, so no restart once it lands.
#   checkpoints:             # one engine per checkpoint (57 §4.1 item 6). A checkpoint is resident only while in use (57 §2.1):
#                            # loaded by the first question (the meeting warms it at Recording), released when the meeting stops /
#                            # the browser session closes, and by idle_unload as the safety net. Nothing loads at daemon start.
#     default: ~             # = model_dir; a bare path, or { dir: <path>, idle_unload: 3m }
#     browser: ~             # cklxx/laya-browser v10, same layout; default <data_dir>/models/decision-browser
#                            # (scripts/fetch-laya-browser.sh until release-models publishes it). A family's `checkpoint: <name>` picks one.
#                            # Its idle_unload defaults to 2m (loop steps are sub-second; a run never trips it).
#   idle_unload: 3m          # drop a checkpoint after this long unused (~0.9 GB back); checkpoints.<name>.idle_unload overrides per checkpoint
#   log_text: false          # true: store the question text in <data_dir>/decisions/*.jsonl, not only its hash
#   families:                # gate per decision family: mode shadow | active | off; accept iff top1 ≥ tau_top and top1 − top2 ≥ tau_margin;
#                            # budget (tier 1, default 400ms) and escalate_budget (tier 2 cheap-model call, default 3s) per family.
#                            # Entries patch this table (any subset of fields); a prefix key sets every family under it:
#                            # `meeting: { mode: active }` is what Settings → Labs (57 §7.1) writes — mode, train and share together.
#                            # Flip a family to active only after its shadow log shows acceptable agreement (57 §2; `decision_stats`).
#                            # train: true (57 §11, default false) captures every batch the family's oracle handled — text, mentions,
#                            # questions, tier-2 answer, outcome, tap/dismiss — to <data_dir>/decisions/training/<prefix>/<day>.jsonl,
#                            # secret-scrubbed, local only; `t --decision-export meeting` folds it into the phrasing-bench fixture shape.
#                            # share: true (57 §7.1, default false) is consent to upload those rows to theuth; needs train (train: false
#                            # clears it); recorded now, honoured by the uploader once the relay endpoint ships (step 11).
#     meeting.kind:    { mode: shadow, tau_top: 0.60, tau_margin: 0.20 }  # small-talk Bool: skip at P ≥ 0.6 with no mentions (57 §3.5)
#     meeting.brief:   { mode: shadow, tau_top: 0.60, tau_margin: 0.30 }  # ≤ 4 cached briefs: publish at P ≥ 0.6, margin ≥ 0.3
#     meeting.subject: { mode: shadow, tau_top: 0.50, tau_margin: 0.00 }  # tier-2 subject extraction rows (cheap model, never the oracle)
#     browser.operation:   { mode: shadow, tau_top: 0.50, tau_margin: 0.00, checkpoint: browser, none: false, cut_ok: true, budget: 2s }
#     browser.target:      { mode: shadow, tau_top: 0.25, tau_margin: 0.00, checkpoint: browser, none: false, cut_ok: true, budget: 2s }
#     browser.type_target: { mode: shadow, tau_top: 0.25, tau_margin: 0.00, checkpoint: browser, none: false, cut_ok: true, budget: 2s }
#                            # ^ the browser System-1 loop's three jev-shaped questions (57 §4.1): `browser: { mode: active }` lets
#                            # `browser_run` act; shadow decides and hands back; off hides the tool. none/cut_ok: the fine-tune
#                            # learned a closed option list and right-cut states, so no `none` control and a cut state is not an error.
#     agent.readable: { mode: shadow, tau_top: 0.85, tau_margin: 0.40 }

# a2a:                       # remote agents over A2A 1.0 (design doc 50 §6); peers/exports live in <data_dir>/a2a/peers.yaml, `/a2a reload` applies edits
#   enabled: false           # false: no actor, listener, card fetch or credential load. Needs agent.communication too.
#   listen: 127.0.0.1:7780   # SDK HTTP listener; loopback only, restart to change
#   public_url: null         # HTTPS base an existing reverse proxy publishes for exports. WARNING: without it the
#                            # listener is plain HTTP for loopback development only; never forward that port to a network.

tools:
  policy:
    read_only: allow
    mutating: ask
    destructive: ask
  # approvals: false   # never prompt; `/approvals off always` writes this
  # pty_shell: ~        # program for `start` PTY jobs; default $SHELL, on Windows pwsh.exe else powershell.exe (58 §0.3)

# secrets:                   # design doc 37; `/secrets`, Settings › Secrets, `t --secret-set`
#   key_store: auto          # auto | keychain | file — where the vault key lives; auto = keychain when one answers, else <data_dir>/secrets.key (0600) with a warning
#   tools: true              # false: no secret_* tools, no prompt block, `{{secret:…}}` stays literal, nothing is scrubbed; the vault, /secrets and the harness's own keys are unaffected
#   request_timeout_secs: 1800   # `secret_request` gives up (the model sees \"no answer\") after this

rpc:
  listen: 127.0.0.1:7777

# signal:                   # phone frontend; runs in `t --headless`. Pair with `t --link-signal`.
#   enabled: true           # daemon starts the channel when linked
#   device_name: theuth        # shown in the phone's Linked devices list
#   owner: \"+15551234567\"   # set by --link-signal / signal_link; owner-only commands
#   allowed: []             # more E.164s; each gets its own thread

# slack:                    # Slack frontend; runs in `t --headless`. `t --slack-setup` creates the app and fills
#                           # this in; `t --slack-manifest` prints the manifest for the manual path (tokens → `t --secret-set slack_bot` / `slack_app`).
#   enabled: true           # daemon opens the socket when tokens are present
#   owner: U0123456789      # owner-only commands; may click any approval; receives unbound approvals
#   allowed: []             # more user ids; each owns their threads
#   channels: []            # channel ids where @theuth is honoured; empty = DMs only
#   tools: all              # all | errors | none — task cards per tool call
#   reasoning: true         # post the quoted reasoning message
#   approvals: all          # all | threads — approvals for sessions not bound to a thread go to the owner DM (all) or nowhere (threads)
";

const DEFAULT_AGENTS: &str =
    "# External harnesses runnable as subagents over the Agent Client Protocol
# (spawn_subagent{agent: <name>}). `/acp` lists, probes and adds entries from the ACP registry.
# opencode:
#   command: opencode
#   args: [acp]
#   description: OpenCode, local install
# codex-acp:
#   command: npx
#   args: [-y, \"@agentclientprotocol/codex-acp\"]
";

/// The bundled `harness` skill (design doc 07 §4): what the agent needs to
/// operate its own harness, loaded on demand through the skills index.
const HARNESS_SKILL: &str = include_str!("../harness.md");

/// Creates the data-dir skeleton on first run. Existing files are never
/// overwritten, except `skills/harness.md`, `skills/visualize.md`,
/// `skills/pdf.md`, `skills/loops.md` and `skills/apps.md`, which the binary
/// owns and rewrites so they always describe the running version.
fn first_run(data_dir: &Path, config_path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(data_dir.join("mcp"))?;
    std::fs::create_dir_all(data_dir.join("acp"))?;
    let skills = data_dir.join("skills");
    std::fs::create_dir_all(&skills)?;
    for (name, body) in [
        ("harness.md", HARNESS_SKILL.to_owned()),
        ("visualize.md", surface::visualize_skill()),
        ("pdf.md", artifact::pdf::skill()),
        ("loops.md", schedule::tools::LOOPS_SKILL.to_owned()),
        ("apps.md", app::skill()),
    ] {
        let path = skills.join(name);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(body.as_str()) {
            std::fs::write(path, body)?;
        }
    }
    let agents = data_dir.join("acp").join("agents.yaml");
    if !agents.exists() {
        std::fs::write(agents, DEFAULT_AGENTS)?;
    }
    if !config_path.exists() {
        std::fs::write(config_path, DEFAULT_CONFIG)?;
    }
    let prompt = data_dir.join("prompt.md");
    if !prompt.exists() {
        std::fs::write(
            prompt,
            "<!-- Standing instructions for the assistant; read at every model call. -->\n",
        )?;
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    // Before clap, tokio, logging or anything that could touch stdout: the
    // worker's stdout is the protocol channel (08 §9).
    if std::env::args().nth(1).as_deref() == Some("subprocess") {
        std::process::exit(i32::from(worker::run()));
    }
    let result = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run());
    // A failed start (a plugin refused) still unlinks engine.json (63 §2.1).
    rpc::engine::release();
    result
}

async fn run() -> anyhow::Result<()> {
    // reqwest links `ring`, slack/relay link `aws-lc-rs`; with both present rustls
    // panics on the first TLS dial unless a process provider is chosen (21 §2.1).
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let cli = Cli::parse();
    if cli.slack_manifest {
        print!("{}", slack::MANIFEST);
        return Ok(());
    }
    if cli.app_parser_hash {
        println!("{}", app::HASH.trim());
        return Ok(());
    }

    let data_dir = cli
        .data_dir
        .or_else(|| dirs::home_dir().map(|h| h.join(".theuth")))
        .ok_or_else(|| anyhow::anyhow!("cannot determine data dir; pass --data-dir"))?;

    let config_path = cli.config.unwrap_or_else(|| data_dir.join("config.yaml"));
    first_run(&data_dir, &config_path)?;
    let config = Figment::new()
        .merge(Yaml::file(&config_path))
        .merge(Env::prefixed("THEUTH_").split("__"));
    // 58 step 12: one process-wide knob every model loader reads; set before any plugin starts.
    let models_cfg: models::accelerator::ModelsConfig =
        config.extract_inner("models").unwrap_or_default();
    models::accelerator::set(models_cfg.accelerator);
    models::onnx::prefer_webgpu_adapter(models_cfg.webgpu_adapter);
    if cli.bench_models {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_writer(std::io::stderr)
            .try_init();
        let decision_cfg: decision::DecisionConfig =
            config.extract_inner("decision").unwrap_or_default();
        let accel = cli.accelerator.unwrap_or(models_cfg.accelerator);
        let code = tokio::task::spawn_blocking(move || {
            bench_models::run(&data_dir, &decision_cfg, accel)
        })
        .await?;
        std::process::exit(code);
    }
    if cli.update {
        let cfg: update::UpdateConfig = config.extract_inner("update").unwrap_or_default();
        return update_cli(&cfg, &data_dir).await;
    }
    if let Some(prefix) = cli.decision_export {
        let format: decision::training::Format = cli.format.parse().map_err(anyhow::Error::msg)?;
        let store = decision::training::Store::new(data_dir.join("decisions").join("training"));
        let v = decision::training::export(&store, &prefix, cli.since, format)?;
        let n = match &v {
            serde_json::Value::Array(a) => a.len(),
            other => other["batches"].as_array().map_or(0, Vec::len),
        };
        let text = serde_json::to_string_pretty(&v)?;
        match cli.out {
            Some(path) => {
                std::fs::write(&path, text)?;
                eprintln!("wrote {n} examples from {} to {}", store.family_dir(&prefix).display(), path.display());
            }
            None => println!("{text}"),
        }
        return Ok(());
    }
    if let Some(name) = cli.secret_set {
        let op = secret_cli::Op::Set {
            name,
            note: cli.secret_note,
        };
        return secret_cli::run(&data_dir, &config, op);
    }
    if let Some(name) = cli.secret_rm {
        return secret_cli::run(&data_dir, &config, secret_cli::Op::Rm(name));
    }
    if cli.secret_list {
        return secret_cli::run(&data_dir, &config, secret_cli::Op::List);
    }
    if cli.slack_setup {
        return slack::setup::run(&data_dir, cli.app, &mut slack::setup::Terminal).await;
    }
    if cli.link {
        return link_cli::run(&data_dir, &config).await;
    }
    if cli.install_service {
        return service::install(
            &data_dir,
            &config_path,
            &config,
            service::Opts {
                relay: cli.relay,
                name: cli.name,
                pair: !cli.no_pair,
            },
        )
        .await;
    }
    if cli.uninstall_service {
        return service::uninstall().await;
    }

    // 63 §0.1: one engine per data dir. The lock decides; a refusal names
    // the holder. `THEUTH_LAUNCH` marks the desktop app's daemon (18 §4).
    use rpc::engine::Host;
    let host = if cli.headless {
        if std::env::var_os("THEUTH_LAUNCH").is_some() {
            Host::Desktop
        } else {
            Host::Headless
        }
    } else if cli.link_signal {
        Host::Signal
    } else if cli.acp {
        Host::Acp
    } else if cli.prompt.is_some() {
        Host::Prompt
    } else {
        Host::Tui
    };
    // Every engine serves (63 §6.4): the configured address, or — for a
    // TUI/-p/--acp without `--serve` — an ephemeral port recorded once bound.
    let listen = if cli.serve || cli.headless {
        Some(link_cli::listen(&config))
    } else {
        config.find_value("rpc.listen").ok().and_then(|v| v.into_string())
    };
    // `Some` = attached to the running engine (63 §0.3): TUI, `-p`, `--acp`.
    let attached = match rpc::engine::acquire(&data_dir, host, listen) {
        Ok(lease) => {
            rpc::engine::hold(lease);
            None
        }
        Err(rpc::engine::Error::Running(running)) => {
            let attach = matches!(host, Host::Tui | Host::Prompt | Host::Acp);
            if !attach || running.url.is_none() {
                eprintln!("{running}");
                std::process::exit(3);
            }
            match rpc::client::connect(&running).await {
                Ok(ws) => Some((std::sync::Arc::new(ws), running)),
                Err(e) => {
                    eprintln!("{running}\ncould not attach: {e:#}");
                    std::process::exit(3);
                }
            }
        }
        Err(e) => return Err(e.into()),
    };

    // The TUI owns the terminal; logs go to a file.
    let logs = data_dir.join("logs");
    std::fs::create_dir_all(&logs)?;
    let (writer, _guard) =
        tracing_appender::non_blocking(tracing_appender::rolling::daily(&logs, "theuth.log"));
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive("info".parse()?),
        )
        .with_writer(writer)
        .with_ansi(false)
        .init();
    // A panic's message goes to stderr, which the TUI owns in raw mode: log it too.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("panic: {info}");
        default_hook(info);
    }));

    // V8's one-time setup must happen on the main thread before any plugin
    // thread builds an isolate, and the env knobs are exported while the
    // process is still effectively single-threaded (23 §3.1).
    let browser_cfg = config
        .extract_inner::<browser::BrowserConfig>("browser")
        .unwrap_or_default();
    browser::warmup(&browser_cfg);

    let rt = Runtime::new(config, data_dir);
    let attached_client = attached.is_some();

    // Pairing needs no TUI, agent or tools; the plugin cancels `shutdown`
    // when the phone has scanned (12 §3).
    let plugins: Vec<Box<dyn Plugin>> = if let Some((ws, running)) = attached {
        // A client of the running engine: no engine plugins here (63 §2.3).
        if cli.acp {
            vec![Box::new(rpc::AcpPlugin { remote: Some(ws) })]
        } else {
            let prompt = match cli.prompt {
                Some(p) if p.is_empty() => Some(std::io::read_to_string(std::io::stdin())?),
                p => p,
            };
            // Takeover (63 §2.3): the engine this process would have run —
            // serving RPC, so the desktop app finds it again (§4.2).
            let start_engine: tui::StartEngine = Box::new(|rt| {
                Box::pin(async move { start_plugins(&rt, engine_plugins(false, false)).await })
            });
            vec![Box::new(tui::TuiPlugin {
                resume: cli.resume,
                prompt,
                attached: Some(tui::Attached {
                    client: ws,
                    running,
                    start_engine,
                }),
            })]
        }
    } else if cli.link_signal {
        vec![Box::new(signal::SignalPlugin {
            channel: false,
            link: Some(signal::LinkOpts {
                owner: cli.owner,
                device_name: cli.device_name,
            }),
        })]
    } else if cli.acp {
        // stdout is the protocol channel: no TUI, no line printer; tracing
        // already goes to the file (06 §8.13).
        vec![
            Box::new(tools::ToolsPlugin),
            Box::new(secrets::SecretsPlugin),
            Box::new(document::DocumentPlugin),
            Box::new(knowledge::KnowledgePlugin),
            Box::new(web::WebPlugin),
            Box::new(browser::BrowserPlugin),
            Box::new(scrape::ScrapePlugin),
            Box::new(codeindex::CodeindexPlugin),
            Box::new(routes::RoutesPlugin),
            Box::new(computer::ComputerPlugin),
            Box::new(artifact::ArtifactPlugin),
            Box::new(app::AppPlugin),
            Box::new(search_providers::SearchProvidersPlugin),
            Box::new(agent::AgentPlugin),
            // The `search` tool (49 §4.4): registration only.
            Box::new(rpc::SearchToolsPlugin),
            // 63 §6.4: attachable, on a loopback port; stdout untouched.
            Box::new(rpc::RpcPlugin { implicit: true }),
            Box::new(signal::SignalPlugin {
                channel: false,
                link: None,
            }),
            Box::new(slack::SlackPlugin { channel: false }),
            Box::new(rpc::AcpPlugin { remote: None }),
        ]
    } else {
        let mut plugins = engine_plugins(cli.headless, cli.serve || cli.headless);
        if !cli.headless {
            let prompt = match cli.prompt {
                Some(p) if p.is_empty() => Some(std::io::read_to_string(std::io::stdin())?),
                p => p,
            };
            plugins.push(Box::new(tui::TuiPlugin {
                resume: cli.resume,
                prompt,
                attached: None,
            }));
        }
        plugins
    };

    // The pairing-only run and an attached client have no registry to
    // expect participants on.
    if cli.link_signal || attached_client {
        for plugin in plugins {
            let name = plugin.name();
            plugin
                .start(&rt)
                .await
                .map_err(|e| anyhow::anyhow!("plugin {name} failed to start: {e:#}"))?;
            tracing::info!(plugin = name, "started");
        }
    } else {
        start_plugins(&rt, plugins).await?;
    }

    tokio::select! {
        _ = rt.shutdown.cancelled() => {}
        _ = tokio::signal::ctrl_c() => rt.shutdown.cancel(),
        _ = parent_gone() => rt.shutdown.cancel(),
    }
    // Give actors a moment to flush (sqlite writes, terminal restore), then
    // leave without waiting for the runtime: a `spawn_blocking` pipe drain
    // whose child never closed its end would otherwise pin the process.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    drop(_guard);
    rpc::engine::release();
    std::process::exit(0)
}

/// Resolves once the process named by `THEUTH_PARENT_PID` is gone; never when
/// it is unset. The Windows desktop app sets it on its daemon (58 §0.4): the
/// app's Job Object kills us when it exits, this is the belt to that braces
/// (a job the app failed to close, a pid_alive-visible crash). Polled, 5 s.
async fn parent_gone() {
    let Some(pid) = std::env::var("THEUTH_PARENT_PID")
        .ok()
        .and_then(|p| p.parse::<u32>().ok())
    else {
        return std::future::pending().await;
    };
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        if !platform::process::pid_alive(pid) {
            tracing::info!(parent = pid, "desktop app gone; shutting down");
            return;
        }
    }
}

/// The engine's plugin list (every mode but `--acp` and `--link-signal`):
/// channels run in `--headless`; the RPC server always (63 §6.4) — on the
/// configured address with `serve`, else implicitly (ephemeral fallback).
fn engine_plugins(headless: bool, serve: bool) -> Vec<Box<dyn Plugin>> {
    let mut plugins: Vec<Box<dyn Plugin>> = vec![
        Box::new(tools::ToolsPlugin),
        Box::new(secrets::SecretsPlugin),
        Box::new(document::DocumentPlugin),
        Box::new(knowledge::KnowledgePlugin),
        Box::new(web::WebPlugin),
        Box::new(browser::BrowserPlugin),
        Box::new(scrape::ScrapePlugin),
        Box::new(codeindex::CodeindexPlugin),
        Box::new(routes::RoutesPlugin),
        Box::new(computer::ComputerPlugin),
        Box::new(artifact::ArtifactPlugin),
        Box::new(app::AppPlugin),
        Box::new(search_providers::SearchProvidersPlugin),
        Box::new(agent::AgentPlugin),
        Box::new(rpc::SearchToolsPlugin),
        Box::new(signal::SignalPlugin {
            channel: headless,
            link: None,
        }),
        Box::new(slack::SlackPlugin { channel: headless }),
        Box::new(link::LinkPlugin),
        Box::new(nodes::NodesPlugin),
        Box::new(schedule::SchedulePlugin),
        // Off unless `a2a.enabled: true`: then it registers its actor and
        // finishes in the background once `agent` and `secrets` answer, so
        // its place in this list carries no ordering promise (50 §11 row 3).
        Box::new(a2a::A2aPlugin),
        Box::new(update::UpdatePlugin),
        Box::new(voice::VoicePlugin),
        Box::new(decision::DecisionPlugin),
    ];
    // 58 §6.1: capture backends per OS; `MeetingStatus.supported` says what this host can do.
    plugins.push(Box::new(meeting::MeetingPlugin));
    plugins.push(Box::new(rpc::RpcPlugin { implicit: !serve }));
    plugins
}

/// Start `plugins` on `rt` in order. First, 51 §8.2: the owners the registry
/// must hear from before a private root may open — every lifecycle
/// participant whose plugin runs here (each plugin honours its own
/// `<section>.enabled`, default on). Jobs is registry-internal.
async fn start_plugins(rt: &Runtime, plugins: Vec<Box<dyn Plugin>>) -> anyhow::Result<()> {
    let owners: Vec<&'static str> = ["artifact", "app", "browser", "computer", "voice", "routes"]
        .into_iter()
        .filter(|name| {
            rt.config
                .extract_inner::<bool>(&format!("{name}.enabled"))
                .unwrap_or(true)
        })
        .collect();
    let registry_rt = rt.clone();
    tokio::spawn(async move {
        if let Some(reg) = registry_rt
            .lookup_when::<tools::ToolRegistry>(tools::REGISTRY_ACTOR)
            .await
        {
            let _ = reg.tell(tools::ExpectParticipants(owners)).await;
        }
    });
    for plugin in plugins {
        let name = plugin.name();
        plugin
            .start(rt)
            .await
            .map_err(|e| anyhow::anyhow!("plugin {name} failed to start: {e:#}"))?;
        tracing::info!(plugin = name, "started");
    }
    Ok(())
}

/// `t --update` (39 §3.2): one foreground check, one line on stdout.
/// Logs go to stderr so `RUST_LOG=debug t --update` shows the debug lines.
async fn update_cli(cfg: &update::UpdateConfig, data_dir: &Path) -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init();
    match update::run_once(cfg, data_dir).await {
        Ok(update::Outcome::UpToDate) => println!("up to date ({})", env!("CARGO_PKG_VERSION")),
        Ok(update::Outcome::Applied { from, to }) => println!("theuth {from} → {to}"),
        Ok(update::Outcome::Available { version, path }) => println!(
            "theuth {version} available; {} is not writable — run `sudo t --update`",
            path.display()
        ),
        Err(e) => {
            eprintln!("update: {e:#}");
            std::process::exit(1);
        }
    }
    Ok(())
}