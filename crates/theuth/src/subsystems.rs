//! Subsystem gating (design doc 224): which plugins an engine starts for its role.
//!
//! Every gated subsystem declares what it needs ([`Need`]); [`decide`] weighs that against
//! the host ([`Env`]) and the owner's override (`subsystems.<name>: auto|on|off`). A subsystem
//! whose `<name>.enabled` is `false` is started anyway — its own start runs the disabled path
//! (dreaming switches its routine off) — and only reported as skipped. Hub-only subsystems are
//! started by [`follow_hub`] on a scoped runtime and stopped when the lease moves.
//! Ungated plugins (the agent, tools, mesh, replica, …) always start.

use std::collections::BTreeMap;
use std::time::Duration;

use runtime::{Plugin, Runtime, Scope};
use serde::Deserialize;
use tokio::sync::watch;

/// What a subsystem needs from the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// Runs only while this engine holds the hub lease (67 §4.4).
    Hub,
    /// A display reachable now (X11/Wayland on Linux; always on macOS and Windows) —
    /// a virtual one (Xvfb) counts; `--headless` does not matter.
    Display,
    /// A person at the machine with a microphone: an interactive host (desktop app, TUI),
    /// never a `t --headless` node, and an audio device.
    User,
}

/// `subsystems.<name>` in config.yaml.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Override {
    /// Follow the subsystem's rule.
    #[default]
    Auto,
    /// Start regardless of the rule (a hub-only subsystem runs on every engine).
    On,
    /// Never start.
    Off,
}

impl<'de> Deserialize<'de> for Override {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Bool(bool),
            Text(String),
        }
        match Raw::deserialize(d)? {
            Raw::Bool(true) => Ok(Self::On),
            Raw::Bool(false) => Ok(Self::Off),
            Raw::Text(t) => match t.trim().to_ascii_lowercase().as_str() {
                "auto" => Ok(Self::Auto),
                "on" | "true" => Ok(Self::On),
                "off" | "false" => Ok(Self::Off),
                other => Err(serde::de::Error::custom(format!(
                    "subsystems: `{other}` is not auto, on or off"
                ))),
            },
        }
    }
}

/// One gated subsystem: its plugin name (= its config section) and its needs.
#[derive(Clone, Copy, Debug)]
pub struct Rule {
    pub name: &'static str,
    pub needs: &'static [Need],
}

/// The gating table (224 §2). Plugins not listed always start.
pub const RULES: &[Rule] = &[
    Rule { name: "dreaming", needs: &[Need::Hub] },
    Rule { name: "signal", needs: &[Need::Hub] },
    Rule { name: "slack", needs: &[Need::Hub] },
    Rule { name: "telegram", needs: &[Need::Hub] },
    Rule { name: "meeting", needs: &[Need::User, Need::Display] },
    Rule { name: "voice", needs: &[Need::User] },
    Rule { name: "talk", needs: &[Need::User] },
    Rule { name: "computer", needs: &[Need::Display] },
];

/// The channel plugins: off the hub they still start, control-only (`channel: false`: the
/// actor and tools, no listener), so status/linking RPCs answer.
pub const CHANNELS: [&str; 3] = ["signal", "slack", "telegram"];

/// What the host offers, probed once at start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Env {
    /// Desktop app daemon, TUI, `-p`, `--acp`: a person drives this engine. `t --headless`
    /// outside the desktop app is not interactive.
    pub interactive: bool,
    /// The engine runs the channel listeners at all (`--headless`, the desktop daemon).
    pub daemon: bool,
    pub display: bool,
    pub audio: bool,
}

impl Env {
    pub fn probe(host: rpc::engine::Host, daemon: bool) -> Self {
        Self {
            interactive: !matches!(host, rpc::engine::Host::Headless),
            daemon,
            display: display_available(),
            audio: audio_available(),
        }
    }
}

/// The verdict for one subsystem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Start with the other plugins.
    Start,
    /// Do not start; the reason goes in the startup line.
    Skip(String),
    /// Started and stopped by [`follow_hub`] with the lease.
    FollowHub,
}

/// Decide `rule` for `env` under `ov`.
pub fn decide(rule: &Rule, ov: Override, env: &Env) -> Decision {
    match ov {
        Override::Off => return Decision::Skip(format!("subsystems.{}: off", rule.name)),
        Override::On => return Decision::Start,
        Override::Auto => {}
    }
    for need in rule.needs {
        match need {
            Need::Display if !env.display => {
                return Decision::Skip("no display".into());
            }
            Need::User if !env.interactive => {
                return Decision::Skip("headless: no user or microphone".into());
            }
            Need::User if !env.audio => return Decision::Skip("no audio device".into()),
            Need::Hub if CHANNELS.contains(&rule.name) && !env.daemon => {
                // Terminal mode never runs a listener: control-only, as before.
                return Decision::Start;
            }
            Need::Hub => return Decision::FollowHub,
            _ => {}
        }
    }
    Decision::Start
}

/// The `subsystems:` section; absent or malformed = every override `auto`.
pub fn overrides(rt: &Runtime) -> BTreeMap<String, Override> {
    rt.section("subsystems").unwrap_or_else(|e| {
        if rt.config.find_value("subsystems").is_ok() {
            tracing::warn!("subsystems: ignored, every subsystem on auto: {e}");
        }
        BTreeMap::new()
    })
}

/// Every plugin name's decision, gated ones from [`RULES`], the rest [`Decision::Start`].
pub fn plan(names: &[&'static str], ov: &BTreeMap<String, Override>, env: &Env) -> Vec<(&'static str, Decision)> {
    names
        .iter()
        .map(|&name| {
            let d = RULES.iter().find(|r| r.name == name).map_or(Decision::Start, |r| {
                decide(r, ov.get(name).copied().unwrap_or_default(), env)
            });
            (name, d)
        })
        .collect()
}

/// The one startup line: gated subsystems only (ungated plugins are logged one by one).
/// `disabled` names those whose own `<name>.enabled` is false.
pub fn summary(plan: &[(&'static str, Decision)], disabled: &[&str], env: &Env) -> String {
    let mut started = Vec::new();
    let mut skipped = Vec::new();
    let mut hub = Vec::new();
    for (name, d) in plan {
        if !RULES.iter().any(|r| r.name == *name) {
            continue;
        }
        if disabled.contains(name) {
            skipped.push(format!("{name} ({name}.enabled: false)"));
            continue;
        }
        match d {
            Decision::Start if CHANNELS.contains(name) && !env.daemon => {
                started.push(format!("{name} (control only)"))
            }
            Decision::Start => started.push((*name).to_owned()),
            Decision::Skip(why) => skipped.push(format!("{name} ({why})")),
            Decision::FollowHub => hub.push(*name),
        }
    }
    let none = |v: &[String]| if v.is_empty() { "-".to_owned() } else { v.join(", ") };
    format!(
        "subsystems: started [{}]; skipped [{}]; hub-only, following the lease [{}] (interactive={}, display={}, audio={})",
        none(&started),
        none(&skipped),
        if hub.is_empty() { "-".to_owned() } else { hub.join(", ") },
        env.interactive,
        env.display,
        env.audio,
    )
}

/// Builds the plugin a hub-only subsystem runs in one lease phase: `make(true)` on the hub,
/// `make(false)` elsewhere; `None` = nothing in that phase.
pub type MakePlugin = Box<dyn Fn(bool) -> Option<Box<dyn Plugin>> + Send + Sync>;

/// A hub-only subsystem under [`follow_hub`].
pub struct HubSubsystem {
    pub name: &'static str,
    pub make: MakePlugin,
}

/// Timing of [`follow_hub`].
#[derive(Clone, Copy, Debug)]
pub struct HubTiming {
    /// How long to wait for the mesh's first lease before falling back to `mesh::is_hub()`
    /// (no mesh, or it failed to start).
    pub first_lease: Duration,
    /// A new lease value must hold this long before subsystems move (no churn on a flap).
    pub settle: Duration,
}

impl Default for HubTiming {
    fn default() -> Self {
        Self {
            first_lease: Duration::from_secs(15),
            settle: Duration::from_secs(5),
        }
    }
}

/// Start each subsystem's phase for the current lease, then follow it: on a settled change
/// stop the old phase (its scope; [`Scope::stop`] waits until its actors are gone) and start
/// the new one. Ends with the process (`rt.shutdown`).
pub async fn follow_hub(
    rt: Runtime,
    subs: Vec<HubSubsystem>,
    mut lease: watch::Receiver<Option<bool>>,
    fallback: impl Fn() -> bool,
    timing: HubTiming,
) {
    if subs.is_empty() {
        return;
    }
    let first = tokio::select! {
        _ = rt.shutdown.cancelled() => return,
        r = tokio::time::timeout(timing.first_lease, lease.wait_for(Option::is_some)) => match r {
            Ok(Ok(v)) => v.unwrap_or(true),
            _ => {
                let hub = fallback();
                tracing::info!(hub, "subsystems: no lease from the mesh; hub-only subsystems follow is_hub()");
                hub
            }
        },
    };
    let mut running: Vec<Option<Scope>> = subs.iter().map(|_| None).collect();
    let mut hub = first;
    apply(&rt, &subs, &mut running, hub).await;
    loop {
        tokio::select! {
            _ = rt.shutdown.cancelled() => return,
            changed = lease.changed() => if changed.is_err() {
                // The sender is static; a closed channel means no further changes.
                return std::future::pending().await;
            },
        }
        // Settle: act only on a value that held for `settle`.
        tokio::select! {
            _ = rt.shutdown.cancelled() => return,
            _ = tokio::time::sleep(timing.settle) => {}
        }
        let Some(now) = *lease.borrow_and_update() else {
            continue;
        };
        if now != hub {
            hub = now;
            apply(&rt, &subs, &mut running, hub).await;
        }
    }
}

/// Move every subsystem to its `hub` phase.
async fn apply(rt: &Runtime, subs: &[HubSubsystem], running: &mut [Option<Scope>], hub: bool) {
    let mut started = Vec::new();
    let mut stopped = Vec::new();
    for (sub, slot) in subs.iter().zip(running.iter_mut()) {
        if let Some(scope) = slot.take() {
            scope.stop().await;
            stopped.push(sub.name);
        }
        let Some(plugin) = (sub.make)(hub) else {
            continue;
        };
        let (scoped, scope) = rt.scoped();
        match plugin.start(&scoped).await {
            Ok(()) => {
                started.push(sub.name);
                *slot = Some(scope);
            }
            Err(e) => {
                tracing::warn!("subsystems: {} failed to start: {e:#}", sub.name);
                scope.stop().await;
            }
        }
    }
    tracing::info!(
        hub,
        "subsystems: lease {} → stopped [{}], started [{}]",
        if hub { "held" } else { "elsewhere" },
        stopped.join(", "),
        started.join(", ")
    );
}

/// A display this process can open now. Linux/BSD: a Wayland socket or an X11 display whose
/// socket exists (a TCP `host:N` display counts); elsewhere the platform always has one.
fn display_available() -> bool {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let var = |k| std::env::var(k).ok().filter(|v: &String| !v.is_empty());
        if let Some(w) = var("WAYLAND_DISPLAY") {
            let path = if w.starts_with('/') {
                std::path::PathBuf::from(&w)
            } else {
                var("XDG_RUNTIME_DIR").map_or_else(std::path::PathBuf::new, std::path::PathBuf::from).join(&w)
            };
            if path.exists() {
                return true;
            }
        }
        if let Some(d) = var("DISPLAY") {
            return match d.split_once(':') {
                Some(("", n)) | Some(("unix", n)) => {
                    let n = n.split('.').next().unwrap_or("");
                    std::path::Path::new(&format!("/tmp/.X11-unix/X{n}")).exists()
                }
                Some(_) => true,
                None => false,
            };
        }
        false
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        true
    }
}

/// An audio device: on Linux/BSD an ALSA device node or a PulseAudio/PipeWire socket;
/// elsewhere assumed.
fn audio_available() -> bool {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let alsa = std::fs::read_dir("/dev/snd")
            .map(|d| d.flatten().any(|e| e.file_name().to_string_lossy().starts_with("pcm")))
            .unwrap_or(false);
        let server = std::env::var_os("XDG_RUNTIME_DIR").is_some_and(|d| {
            let d = std::path::PathBuf::from(d);
            d.join("pulse/native").exists() || d.join("pipewire-0").exists()
        });
        alsa || server || std::env::var_os("PULSE_SERVER").is_some()
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const NODE: Env = Env { interactive: false, daemon: true, display: false, audio: false };
    const DESKTOP: Env = Env { interactive: true, daemon: true, display: true, audio: true };
    const XVFB_NODE: Env = Env { interactive: false, daemon: true, display: true, audio: false };
    const TUI: Env = Env { interactive: true, daemon: false, display: true, audio: true };

    fn rule(name: &str) -> &'static Rule {
        RULES.iter().find(|r| r.name == name).unwrap()
    }

    fn d(name: &str, ov: Override, env: &Env) -> Decision {
        decide(rule(name), ov, env)
    }

    #[test]
    fn a_headless_node_runs_no_user_or_display_subsystem() {
        for name in ["meeting", "voice", "talk", "computer"] {
            assert!(matches!(d(name, Override::Auto, &NODE), Decision::Skip(_)), "{name}");
        }
        for name in ["dreaming", "signal", "slack", "telegram"] {
            assert_eq!(d(name, Override::Auto, &NODE), Decision::FollowHub, "{name}");
        }
    }

    #[test]
    fn a_virtual_display_enables_computer_use_but_not_meetings() {
        assert_eq!(d("computer", Override::Auto, &XVFB_NODE), Decision::Start);
        assert!(matches!(d("meeting", Override::Auto, &XVFB_NODE), Decision::Skip(_)));
        assert!(matches!(d("voice", Override::Auto, &XVFB_NODE), Decision::Skip(_)));
    }

    #[test]
    fn the_desktop_runs_everything_and_follows_the_hub_for_hub_duties() {
        for name in ["meeting", "voice", "talk", "computer"] {
            assert_eq!(d(name, Override::Auto, &DESKTOP), Decision::Start, "{name}");
        }
        assert_eq!(d("dreaming", Override::Auto, &DESKTOP), Decision::FollowHub);
        assert_eq!(d("signal", Override::Auto, &DESKTOP), Decision::FollowHub);
    }

    #[test]
    fn terminal_mode_keeps_channels_control_only_and_dreaming_hub_only() {
        assert_eq!(d("slack", Override::Auto, &TUI), Decision::Start);
        assert_eq!(d("dreaming", Override::Auto, &TUI), Decision::FollowHub);
    }

    #[test]
    fn overrides_beat_the_rule() {
        assert_eq!(d("meeting", Override::On, &NODE), Decision::Start);
        assert_eq!(d("dreaming", Override::On, &NODE), Decision::Start);
        assert!(matches!(d("computer", Override::Off, &DESKTOP), Decision::Skip(_)));
        assert!(matches!(d("signal", Override::Off, &DESKTOP), Decision::Skip(_)));
    }

    #[test]
    fn a_linux_box_without_audio_skips_voice_even_in_the_tui() {
        let env = Env { audio: false, ..TUI };
        assert!(matches!(d("voice", Override::Auto, &env), Decision::Skip(_)));
    }

    #[test]
    fn overrides_parse_from_yaml_words_and_bools() {
        use figment::providers::{Format, Yaml};
        let fig = figment::Figment::new().merge(Yaml::string(
            "subsystems:\n  meeting: on\n  voice: off\n  talk: auto\n  computer: true\n  dreaming: false\n",
        ));
        let rt = Runtime::new(fig, std::env::temp_dir());
        let ov = overrides(&rt);
        assert_eq!(ov["meeting"], Override::On);
        assert_eq!(ov["voice"], Override::Off);
        assert_eq!(ov["talk"], Override::Auto);
        assert_eq!(ov["computer"], Override::On);
        assert_eq!(ov["dreaming"], Override::Off);
    }

    #[test]
    fn plan_and_summary_cover_gated_names_only() {
        let names = ["agent", "dreaming", "meeting", "computer", "slack"];
        let p = plan(&names, &BTreeMap::new(), &NODE);
        assert_eq!(p[0], ("agent", Decision::Start));
        let line = summary(&p, &["computer"], &NODE);
        assert_eq!(
            line,
            "subsystems: started [-]; skipped [meeting (headless: no user or microphone), computer (computer.enabled: false)]; hub-only, following the lease [dreaming, slack] (interactive=false, display=false, audio=false)"
        );
    }

    /// Counts starts and live scopes; a start while its previous scope is alive fails, the
    /// way a second `register` of a still-live named actor would.
    #[derive(Default)]
    struct Probe {
        starts: AtomicUsize,
        live: AtomicUsize,
        hub_phase_starts: AtomicUsize,
    }

    struct Fake {
        probe: Arc<Probe>,
        hub: bool,
    }

    #[async_trait::async_trait]
    impl Plugin for Fake {
        fn name(&self) -> &'static str {
            "fake"
        }
        async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
            anyhow::ensure!(self.probe.live.load(Ordering::SeqCst) == 0, "previous scope still live");
            self.probe.live.fetch_add(1, Ordering::SeqCst);
            self.probe.starts.fetch_add(1, Ordering::SeqCst);
            if self.hub {
                self.probe.hub_phase_starts.fetch_add(1, Ordering::SeqCst);
            }
            let probe = self.probe.clone();
            rt.on_scope_stop(async move {
                tokio::time::sleep(Duration::from_millis(20)).await;
                probe.live.fetch_sub(1, Ordering::SeqCst);
            });
            Ok(())
        }
    }

    fn subs(probe: &Arc<Probe>, hub_only: bool) -> Vec<HubSubsystem> {
        let p = probe.clone();
        vec![HubSubsystem {
            name: "fake",
            make: Box::new(move |hub| {
                (hub || !hub_only).then(|| Box::new(Fake { probe: p.clone(), hub }) as Box<dyn Plugin>)
            }),
        }]
    }

    const FAST: HubTiming = HubTiming {
        first_lease: Duration::from_millis(50),
        settle: Duration::from_millis(30),
    };

    async fn until(what: &str, f: impl Fn() -> bool) {
        for _ in 0..200 {
            if f() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out: {what}");
    }

    #[tokio::test]
    async fn hub_only_subsystems_start_on_the_lease_and_stop_when_it_moves() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let (tx, rx) = watch::channel(None);
        let probe = Arc::new(Probe::default());
        let task = tokio::spawn(follow_hub(rt.clone(), subs(&probe, true), rx, || true, FAST));
        // Not the hub at first: nothing runs.
        tx.send_replace(Some(false));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(probe.starts.load(Ordering::SeqCst), 0);
        // Won the lease: started once.
        tx.send_replace(Some(true));
        until("started on the hub", || probe.live.load(Ordering::SeqCst) == 1).await;
        // Lost it: stopped, and its stop hook ran.
        tx.send_replace(Some(false));
        until("stopped off the hub", || probe.live.load(Ordering::SeqCst) == 0).await;
        // Won it back: started again (the stopped scope was fully released first).
        tx.send_replace(Some(true));
        until("restarted", || probe.starts.load(Ordering::SeqCst) == 2).await;
        assert_eq!(probe.live.load(Ordering::SeqCst), 1);
        rt.shutdown.cancel();
        task.await.unwrap();
    }

    #[tokio::test]
    async fn a_flap_shorter_than_settle_moves_nothing() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let (tx, rx) = watch::channel(Some(true));
        let probe = Arc::new(Probe::default());
        let timing = HubTiming { settle: Duration::from_millis(200), ..FAST };
        let task = tokio::spawn(follow_hub(rt.clone(), subs(&probe, true), rx, || true, timing));
        until("started", || probe.starts.load(Ordering::SeqCst) == 1).await;
        tx.send_replace(Some(false));
        tokio::time::sleep(Duration::from_millis(20)).await;
        tx.send_replace(Some(true));
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(probe.starts.load(Ordering::SeqCst), 1);
        assert_eq!(probe.live.load(Ordering::SeqCst), 1);
        rt.shutdown.cancel();
        task.await.unwrap();
    }

    #[tokio::test]
    async fn channels_swap_between_control_only_and_listening() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let (tx, rx) = watch::channel(Some(false));
        let probe = Arc::new(Probe::default());
        let task = tokio::spawn(follow_hub(rt.clone(), subs(&probe, false), rx, || true, FAST));
        until("control-only start", || probe.starts.load(Ordering::SeqCst) == 1).await;
        assert_eq!(probe.hub_phase_starts.load(Ordering::SeqCst), 0);
        tx.send_replace(Some(true));
        until("listener start", || probe.hub_phase_starts.load(Ordering::SeqCst) == 1).await;
        assert_eq!(probe.live.load(Ordering::SeqCst), 1);
        rt.shutdown.cancel();
        task.await.unwrap();
    }

    #[tokio::test]
    async fn without_a_lease_the_fallback_decides() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let (_tx, rx) = watch::channel(None);
        let probe = Arc::new(Probe::default());
        let task = tokio::spawn(follow_hub(rt.clone(), subs(&probe, true), rx, || true, FAST));
        until("fallback start", || probe.starts.load(Ordering::SeqCst) == 1).await;
        rt.shutdown.cancel();
        task.await.unwrap();
    }
}
