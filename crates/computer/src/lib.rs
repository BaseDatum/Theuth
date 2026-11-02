//! Computer plugin (design doc 27): the model sees and drives the user's
//! desktop through the `computer_*` tools — one composite screenshot of
//! every display as an image part ([`screen`], [`frame`]), enigo pointer and
//! keyboard input on one thread ([`input`]), `Risk` as the only gate
//! ([`tools`]), the accessibility tree with headless element actions over
//! agent-desktop ([`a11y`], 161) and the clipboard ([`clipboard`], 161) —
//! and the platform's native-app tools ([`native`], 153 §11).
//! The [`Computer`] actor exists so RPC has a lookup target.
//!
//! Public surface consumed by other plugins:
//! - commands on the [`COMPUTER_ACTOR`] actor ([`Computer`]): [`Permissions`],
//!   [`ScreenContext`] (96 §6)
//! - [`NATIVE_APPS_SKILL`]: the binary-owned `skills/native-apps.md`

pub mod a11y;
pub mod ax;
pub mod clipboard;
pub mod context;
pub mod frame;
pub mod input;
pub mod native;
mod png;
pub mod pointer;
pub mod screen;
pub mod tools;
mod worker;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ::tools::{
    CloseAck, CloseTree, REGISTRY_ACTOR, RegisterLifecycle, RegisterPrompt, RegisterTool,
    ToolRegistry,
};
use kameo::actor::Spawn;
use kameo::message::{Context, Message};
use runtime::Runtime;
use serde::{Deserialize, Serialize};

/// Registry name of the [`Computer`] actor (27 §3.5).
pub const COMPUTER_ACTOR: &str = "computer";

/// `skills/native-apps.md` (153 §11.4): AppleScript / Shortcuts / PowerShell
/// recipes for the native-app tools, written by `theuth`'s first run.
pub const NATIVE_APPS_SKILL: &str = include_str!("../native-apps.md");

/// `computer:` config section (27 §5). Every field has a default; the section
/// may be absent.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ComputerConfig {
    /// `false`: no actor, no tools, no prompt block, no input thread.
    pub enabled: bool,
    /// Long edge of a screenshot after logical scaling (17 §1; ~1280 for
    /// Anthropic click accuracy).
    pub max_edge: u32,
    /// `screen_context` (96 §6.8).
    pub screen_context: ScreenContextConfig,
}

impl Default for ComputerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_edge: 1568,
            screen_context: ScreenContextConfig::default(),
        }
    }
}

/// `computer.screen_context:` (96 §6.8). Every field has a default.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ScreenContextConfig {
    /// Overview long edge.
    pub max_edge: u32,
    /// A crop is downscaled only above this long edge.
    pub crop_edge: u32,
    /// At most this many crops per call, in gesture precedence.
    pub max_crops: usize,
    /// `off` never calls AX even when granted.
    pub accessibility: AxMode,
    /// Elements reported per gesture.
    pub max_elements: usize,
    /// Label/value length cap in characters.
    pub value_chars: usize,
}

impl Default for ScreenContextConfig {
    fn default() -> Self {
        Self {
            max_edge: 1568,
            crop_edge: 1024,
            max_crops: 3,
            accessibility: AxMode::Auto,
            max_elements: 6,
            value_chars: 80,
        }
    }
}

/// Whether `screen_context` reads the accessibility tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AxMode {
    /// Read it when the client asks and the process is trusted.
    Auto,
    /// Never.
    Off,
}

/// One display as shown by `/computer` and Settings › Computer.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DisplayInfo {
    /// 1-based; what `computer_screenshot { display }` accepts.
    pub index: u32,
    /// OS display name.
    pub name: String,
    /// The primary display.
    pub main: bool,
    /// Logical width.
    pub width: u32,
    /// Logical height.
    pub height: u32,
    /// Physical pixels per logical point.
    pub scale: f32,
}

/// Reply to [`Permissions`]: the two TCC booleans (both `true` off macOS),
/// the attached displays and a note naming the pane and the responsible app.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PermissionStatus {
    /// `AXIsProcessTrusted`: input events will be delivered.
    pub accessibility: bool,
    /// `CGPreflightScreenCaptureAccess`: screenshots will show content.
    pub screen_recording: bool,
    /// Main first, then by position.
    pub displays: Vec<DisplayInfo>,
    /// Where to grant and to which app; empty off macOS.
    pub note: String,
}

/// `ask` → [`PermissionStatus`]. Non-prompting probes (27 §0.8).
#[derive(Clone, Copy, Debug)]
pub struct Permissions;

/// `screen_context` request (96 §6.3): the pointer path in logical global
/// points, the trail colour (`#rrggbb`), the press-to-release span, and
/// whether the client wants the accessibility tree read.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ScreenContextReq {
    /// The pointer path, press to release, in logical global points.
    pub samples: Vec<pointer::Sample>,
    /// Trail colour as `#rrggbb`; unparsable falls back to theuth's green.
    pub color: String,
    /// Press-to-release duration.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub since_ms: u64,
    /// Read the accessibility tree (AND-ed with config and the TCC grant).
    pub accessibility: bool,
}

/// `screen_context` reply: one `[screen …]` line per visited display and the
/// PNGs written under `<data_dir>/attachments/` (overviews, then crops).
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ScreenContextReply {
    /// One `[screen <i>/<n> …]` line per visited display (96 §6.6).
    pub lines: Vec<String>,
    /// Overview PNGs in visit order, then crops; each ≤ 5 MB.
    pub files: Vec<PathBuf>,
}

/// `ask` → `Result<ScreenContextReply, String>` (96 §6.4).
#[derive(Clone, Debug)]
pub struct ScreenContext(pub ScreenContextReq);

/// The [`COMPUTER_ACTOR`] actor. Tools hold [`tools::Ctx`] directly; the
/// actor answers [`Permissions`] and keeps the shared state (and with it the
/// input thread) alive for the process.
#[derive(kameo::Actor)]
pub struct Computer {
    /// Shared tool state.
    pub ctx: Arc<tools::Ctx>,
    /// `<data_dir>`: `screen_context` writes under `attachments/` and `computer/tmp`.
    pub data_dir: PathBuf,
}

impl Message<Permissions> for Computer {
    type Reply = PermissionStatus;

    async fn handle(&mut self, _: Permissions, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let displays = match tokio::task::spawn_blocking(screen::displays).await {
            Ok(Ok(ds)) => ds,
            Ok(Err(e)) => {
                tracing::warn!("display enumeration failed: {e:#}");
                Vec::new()
            }
            Err(e) => {
                tracing::warn!("display enumeration panicked: {e}");
                Vec::new()
            }
        };
        PermissionStatus {
            accessibility: input::accessibility_granted(),
            screen_recording: screen::screen_recording_granted(),
            displays: displays
                .into_iter()
                .map(|d| DisplayInfo {
                    index: d.index,
                    name: d.name,
                    main: d.main,
                    width: d.width,
                    height: d.height,
                    scale: d.scale,
                })
                .collect(),
            note: if cfg!(target_os = "macos") {
                "Grant in System Settings › Privacy & Security to the app that launched theuth (your terminal, or theuth.app). \
                 Screen Recording takes effect after theuth restarts."
                    .to_owned()
            } else {
                String::new()
            },
        }
    }
}

impl Message<ScreenContext> for Computer {
    type Reply = Result<ScreenContextReply, String>;

    /// Capture, stamp, read and write off the actor thread (96 §6.4).
    async fn handle(
        &mut self,
        m: ScreenContext,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let ctx = self.ctx.clone();
        let data_dir = self.data_dir.clone();
        match tokio::task::spawn_blocking(move || {
            context::compose(&m.0, &ctx.cfg.screen_context, &data_dir)
        })
        .await
        {
            Ok(r) => r,
            Err(e) => Err(format!("screen_context panicked: {e}")),
        }
    }
}

impl Message<CloseTree> for Computer {
    type Reply = CloseAck;

    /// A closing tree (51 §4.5): forget its last-screenshot frames and retire
    /// its a11y workers (161 §3.1). Its PNGs sit under the tree's resources
    /// root, which the lifecycle removes.
    async fn handle(&mut self, m: CloseTree, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ctx.forget(&m.subtree);
        CloseAck::ok("computer")
    }
}

/// The plugin (27 §3.1): reads `computer`, spawns [`Computer`] as
/// [`COMPUTER_ACTOR`], registers the tools and the prompt block once the
/// registry is up.
pub struct ComputerPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for ComputerPlugin {
    fn name(&self) -> &'static str {
        "computer"
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        let cfg: ComputerConfig = rt.section("computer").unwrap_or_else(|e| {
            tracing::debug!("no [computer] config section, using defaults: {e}");
            ComputerConfig::default()
        });
        if !cfg.enabled {
            tracing::info!("computer disabled by config");
            return Ok(());
        }
        #[cfg(target_os = "windows")]
        per_monitor_v2_dpi();
        // 161 §3.2: reclaim the namespaces the previous run of this engine left
        // behind, before any worker can exist.
        let a11y = a11y::A11y::new(&rt.data_dir);
        let a11y = tokio::task::spawn_blocking(move || {
            a11y.sweep();
            a11y
        })
        .await?;
        let ctx = Arc::new(tools::Ctx {
            cfg,
            input: input::spawn(),
            frames: Mutex::new(HashMap::new()),
            a11y,
            clipboard: clipboard::Clipboard::default(),
        });
        let actor = Computer::spawn(Computer {
            ctx: ctx.clone(),
            data_dir: rt.data_dir.clone(),
        });
        actor.register(COMPUTER_ACTOR)?;

        let rt = rt.clone();
        tokio::spawn(async move {
            if let Some(reg) = rt.lookup_when::<ToolRegistry>(REGISTRY_ACTOR).await {
                let _ = reg
                    .ask(RegisterLifecycle {
                        owner: "computer",
                        opened: None,
                        close: actor.reply_recipient(),
                    })
                    .await;
                for tool in tools::all(ctx.clone()) {
                    let _ = reg.ask(RegisterTool(tool)).await;
                }
                let _ = reg
                    .ask(RegisterPrompt {
                        plugin: "computer",
                        text: format!("{}\n\n{}", tools::PROMPT, context::PROMPT),
                    })
                    .await;
            }
        });
        Ok(())
    }
}

/// Make the engine per-monitor-v2 DPI-aware on Windows (161 §2), before the
/// input thread, any xcap call, the UIA reader and agent-desktop's bootstrap.
/// 149 WE1 already treats the Windows pointer space as physical pixels (xcap's
/// DEVMODE geometry, `ax.rs`'s `WINDOWS_CAPTURE_RATIO = 1.0`, the companion's
/// PerMonitorV2 manifest); this makes enigo, xcap, `ax.rs` and agent-desktop
/// agree on them, and agent-desktop's own bootstrap then finds awareness set
/// and flips nothing mid-session. At 100 % scaling it changes nothing.
#[cfg(target_os = "windows")]
fn per_monitor_v2_dpi() {
    use windows::Win32::Foundation::ERROR_ACCESS_DENIED;
    use windows::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
    };
    // SAFETY: passes a predefined awareness context and touches no memory of ours.
    match unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) } {
        Ok(()) => tracing::debug!("computer: process DPI awareness set to per-monitor-v2"),
        Err(e) if e.code() == ERROR_ACCESS_DENIED.to_hresult() => {
            tracing::debug!(error = %e, "computer: process DPI awareness was already set");
        }
        Err(e) => tracing::warn!(error = %e, "computer: per-monitor-v2 DPI awareness not set"),
    }
}
