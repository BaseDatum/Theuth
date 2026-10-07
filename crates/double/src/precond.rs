//! Local preconditions (126 §8.2) and the build slot (§8.1). All required for
//! Tier B/C, re-checked every 60 s while waiting and while running; Tier A
//! ignores the window and the power precondition (it is < 2 min of CPU) but
//! still yields to a live meeting / talk / dictation and to a running cargo
//! gate (it takes no slot; it waits while a slot is held by another label).
//!
//! Sensors come through [`Sensors`] so tests inject fakes; [`Live`] is the
//! real one: `platform::idle` / `platform::power` plus one status `ask` to the
//! meeting, talk and dictation actors (`meeting::MeetingStatus` →
//! `Status.state != Idle`; `talk::TalkStatus` → `TalkState.active.is_some()`;
//! `voice::dictation::DictationStatus` → `DictationState.active.is_some()`).
//!
//! Unknown is conservative where the stakes are the user's: an unknown idle
//! time counts as *not idle* and an unknown power source as *battery*, so an
//! engine on Linux/Windows (no probe yet) waits instead of training under the
//! user. Unknown thermal, load, disk and memory pass: they protect the
//! machine, and the kernel throttles on its own.
//!
//! The build slot is `/Users/…/.theuth/bin/build-lock`'s protocol, in Rust:
//! `mkdir <dir>/<i>` (atomic), owner file `"<pid> <label> <epoch> <what>"`,
//! stale when the owner pid is dead, released by removing the directory. With
//! `BUILD_SLOTS=1` only slot 0 exists, so a cargo gate and a training run
//! never overlap on this machine.

use std::path::{Path, PathBuf};

use chrono::{NaiveDateTime, NaiveTime};
use platform::power::{MemoryPressure, Power, Thermal};

use crate::config::Training;
use crate::types::Tier;

/// Default slot directory (the script's `DIR`).
pub const SLOT_DIR: &str = "/tmp/theuth-build-slots";
/// The label a training run writes into its owner file.
pub const TRAIN_LABEL: &str = "double-train";
/// 1-minute load must stay under `cores / 2`.
pub const LOAD_CORES_DIV: f64 = 2.0;
/// Free disk required.
pub const MIN_FREE_DISK: u64 = 10 * 1024 * 1024 * 1024;

/// Which precondition fails.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Blocker {
    /// Outside `training.window`.
    Window,
    /// On battery (or unknown).
    Power,
    /// User active within `idle_min` (or unknown).
    Idle,
    /// A capture is live: `meeting` | `talk` | `dictation`.
    Live(&'static str),
    /// 1-minute load ≥ cores / 2.
    Load,
    /// Thermal state not nominal.
    Thermal,
    /// Free disk < 10 GB.
    Disk,
    /// Memory pressure not normal.
    Memory,
    /// A build slot is held by another label.
    BuildSlot(String),
}

impl std::fmt::Display for Blocker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Window => f.write_str("outside the window"),
            Self::Power => f.write_str("on battery"),
            Self::Idle => f.write_str("user active"),
            Self::Live(what) => write!(f, "{what} live"),
            Self::Load => f.write_str("load high"),
            Self::Thermal => f.write_str("thermal"),
            Self::Disk => f.write_str("disk < 10 GB"),
            Self::Memory => f.write_str("memory pressure"),
            Self::BuildSlot(who) => write!(f, "build slot held by {who}"),
        }
    }
}

/// What the preconditions read. Every method is a point reading.
#[async_trait::async_trait]
pub trait Sensors: Send + Sync {
    /// Local wall-clock date and time.
    fn now_local(&self) -> NaiveDateTime;
    /// Seconds since the last user input; `None` = unknown.
    fn idle_secs(&self) -> Option<f64>;
    /// Mains or battery.
    fn power(&self) -> Power;
    /// Thermal state.
    fn thermal(&self) -> Thermal;
    /// 1-minute load average.
    fn load1(&self) -> Option<f64>;
    /// Logical cores.
    fn cores(&self) -> usize;
    /// Free bytes under the data dir.
    fn free_disk(&self) -> Option<u64>;
    /// Memory pressure.
    fn memory(&self) -> MemoryPressure;
    /// The live capture, if one runs: `meeting` | `talk` | `dictation`.
    async fn live_capture(&self) -> Option<&'static str>;
    /// Labels of build slots held by other (live) owners.
    fn foreign_slots(&self) -> Vec<String>;
}

/// `HH:MM-HH:MM`, wrapping past midnight when `end <= start`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    /// Start, inclusive.
    pub start: NaiveTime,
    /// End, exclusive.
    pub end: NaiveTime,
}

impl Window {
    /// Parse `training.window`; `None` on anything but `HH:MM-HH:MM`.
    pub fn parse(s: &str) -> Option<Self> {
        let (a, b) = s.trim().split_once('-')?;
        let t = |x: &str| NaiveTime::parse_from_str(x.trim(), "%H:%M").ok();
        Some(Self {
            start: t(a)?,
            end: t(b)?,
        })
    }

    /// Whether `t` is inside.
    pub fn contains(&self, t: NaiveTime) -> bool {
        if self.start < self.end {
            t >= self.start && t < self.end
        } else {
            t >= self.start || t < self.end
        }
    }

    /// The window's start on the night `t` belongs to, as minutes since midnight
    /// of a reference: identifies "tonight" for the run cap.
    pub fn night_of(&self, date: chrono::NaiveDate, t: NaiveTime) -> chrono::NaiveDate {
        if self.start >= self.end && t < self.end {
            date.pred_opt().unwrap_or(date)
        } else {
            date
        }
    }
}

/// The failing preconditions for `tier` under `cfg`; empty = go.
pub async fn check(tier: Tier, cfg: &Training, s: &dyn Sensors) -> Vec<Blocker> {
    let mut out = Vec::new();
    if let Some(what) = s.live_capture().await {
        out.push(Blocker::Live(what));
    }
    let foreign = s.foreign_slots();
    if !foreign.is_empty() {
        out.push(Blocker::BuildSlot(foreign.join(",")));
    }
    if tier == Tier::A {
        return out;
    }
    let window = Window::parse(&cfg.window).unwrap_or_else(|| {
        Window::parse(&Training::default().window).expect("default window parses")
    });
    if !window.contains(s.now_local().time()) {
        out.push(Blocker::Window);
    }
    if s.power() != Power::Ac {
        out.push(Blocker::Power);
    }
    if !s
        .idle_secs()
        .is_some_and(|i| i >= f64::from(cfg.idle_min) * 60.0)
    {
        out.push(Blocker::Idle);
    }
    if let Some(l) = s.load1()
        && l >= s.cores().max(1) as f64 / LOAD_CORES_DIV
    {
        out.push(Blocker::Load);
    }
    if !matches!(s.thermal(), Thermal::Nominal | Thermal::Unknown) {
        out.push(Blocker::Thermal);
    }
    if s.free_disk().is_some_and(|b| b < MIN_FREE_DISK) {
        out.push(Blocker::Disk);
    }
    if !matches!(s.memory(), MemoryPressure::Normal | MemoryPressure::Unknown) {
        out.push(Blocker::Memory);
    }
    out
}

// --- the build slot ------------------------------------------------------

/// One held slot; removing the directory releases it (on drop too).
#[derive(Debug)]
pub struct BuildSlot {
    dir: PathBuf,
}

/// `pid label epoch what` as the script writes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    /// Owner process.
    pub pid: u32,
    /// `BUILD_LABEL`.
    pub label: String,
    /// Seconds since the epoch when taken.
    pub epoch: u64,
    /// The command, space-joined.
    pub what: String,
}

impl Owner {
    /// Parse one owner file.
    pub fn parse(s: &str) -> Option<Self> {
        let mut it = s.trim_end_matches('\n').splitn(4, ' ');
        Some(Self {
            pid: it.next()?.parse().ok()?,
            label: it.next()?.to_owned(),
            epoch: it.next()?.parse().ok()?,
            what: it.next().unwrap_or("").to_owned(),
        })
    }

    /// The owner of `slot`, when the file parses.
    pub fn read(slot: &Path) -> Option<Self> {
        Self::parse(&std::fs::read_to_string(slot.join("owner")).ok()?)
    }
}

impl BuildSlot {
    /// Take slot 0 under `dir` (the `BUILD_SLOTS=1` slot): `mkdir` wins or
    /// loses atomically; a slot whose owner pid is dead is reclaimed first.
    /// `Ok(None)` when it is held by a live process.
    pub fn try_acquire(dir: &Path, label: &str, what: &str) -> std::io::Result<Option<Self>> {
        std::fs::create_dir_all(dir)?;
        let slot = dir.join("0");
        if let Err(e) = std::fs::create_dir(&slot) {
            if e.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(e);
            }
            match Owner::read(&slot) {
                Some(o) if platform::process::pid_alive(o.pid) => return Ok(None),
                // Dead owner or unreadable file: stale, as the script treats it.
                _ => {
                    std::fs::remove_dir_all(&slot)?;
                    if std::fs::create_dir(&slot).is_err() {
                        return Ok(None);
                    }
                }
            }
        }
        let epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        std::fs::write(
            slot.join("owner"),
            format!("{} {label} {epoch} {what}\n", std::process::id()),
        )?;
        Ok(Some(Self { dir: slot }))
    }

    /// The slot directory.
    pub fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for BuildSlot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Labels of every slot under `dir` held by a live process whose label is not
/// `own` (what Tier A waits on; stale slots are ignored, not reclaimed).
pub fn foreign_slots(dir: &Path, own: &str) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<String> = rd
        .flatten()
        .filter_map(|e| Owner::read(&e.path()))
        .filter(|o| o.label != own && platform::process::pid_alive(o.pid))
        .map(|o| o.label)
        .collect();
    v.sort();
    v
}

/// The real sensors.
pub struct Live {
    /// Where free disk is measured.
    pub data_dir: PathBuf,
    /// Slot directory.
    pub slots: PathBuf,
}

#[async_trait::async_trait]
impl Sensors for Live {
    fn now_local(&self) -> NaiveDateTime {
        chrono::Local::now().naive_local()
    }
    fn idle_secs(&self) -> Option<f64> {
        platform::idle::idle_secs()
    }
    fn power(&self) -> Power {
        platform::power::power()
    }
    fn thermal(&self) -> Thermal {
        platform::power::thermal()
    }
    fn load1(&self) -> Option<f64> {
        platform::power::load1()
    }
    fn cores(&self) -> usize {
        std::thread::available_parallelism().map_or(1, |n| n.get())
    }
    fn free_disk(&self) -> Option<u64> {
        platform::power::free_disk_bytes(&self.data_dir)
    }
    fn memory(&self) -> MemoryPressure {
        platform::power::memory_pressure()
    }
    async fn live_capture(&self) -> Option<&'static str> {
        use kameo::actor::ActorRef;
        if let Ok(Some(m)) = ActorRef::<meeting::Meeting>::lookup(meeting::MEETING_ACTOR)
            && let Ok(s) = m.ask(meeting::MeetingStatus).await
            && s.state != meeting::MeetingState::Idle
        {
            return Some("meeting");
        }
        if let Ok(Some(t)) = ActorRef::<talk::Talk>::lookup(talk::TALK_ACTOR)
            && let Ok(s) = t.ask(talk::TalkStatus).await
            && s.active.is_some()
        {
            return Some("talk");
        }
        if let Ok(Some(d)) = ActorRef::<voice::Dictation>::lookup(voice::DICTATION_ACTOR)
            && let Ok(s) = d.ask(voice::DictationStatus).await
            && s.active.is_some()
        {
            return Some("dictation");
        }
        None
    }
    fn foreign_slots(&self) -> Vec<String> {
        foreign_slots(&self.slots, TRAIN_LABEL)
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use parking_lot::Mutex;

    /// 2026-10-07 at `h:m` local.
    pub fn at(h: u32, m: u32) -> NaiveDateTime {
        chrono::NaiveDate::from_ymd_opt(2026, 10, 7)
            .unwrap()
            .and_hms_opt(h, m, 0)
            .unwrap()
    }

    /// Every reading settable; green by default.
    #[derive(Debug)]
    pub struct Fake {
        pub now: Mutex<NaiveDateTime>,
        pub idle: Mutex<Option<f64>>,
        pub power: Mutex<Power>,
        pub thermal: Mutex<Thermal>,
        pub load: Mutex<Option<f64>>,
        pub cores: usize,
        pub disk: Mutex<Option<u64>>,
        pub memory: Mutex<MemoryPressure>,
        pub live: Mutex<Option<&'static str>>,
        pub foreign: Mutex<Vec<String>>,
    }

    impl Fake {
        pub fn green() -> Self {
            Self {
                now: Mutex::new(at(3, 0)),
                idle: Mutex::new(Some(20.0 * 60.0)),
                power: Mutex::new(Power::Ac),
                thermal: Mutex::new(Thermal::Nominal),
                load: Mutex::new(Some(1.0)),
                cores: 8,
                disk: Mutex::new(Some(100 << 30)),
                memory: Mutex::new(MemoryPressure::Normal),
                live: Mutex::new(None),
                foreign: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl Sensors for Fake {
        fn now_local(&self) -> NaiveDateTime {
            *self.now.lock()
        }
        fn idle_secs(&self) -> Option<f64> {
            *self.idle.lock()
        }
        fn power(&self) -> Power {
            *self.power.lock()
        }
        fn thermal(&self) -> Thermal {
            *self.thermal.lock()
        }
        fn load1(&self) -> Option<f64> {
            *self.load.lock()
        }
        fn cores(&self) -> usize {
            self.cores
        }
        fn free_disk(&self) -> Option<u64> {
            *self.disk.lock()
        }
        fn memory(&self) -> MemoryPressure {
            *self.memory.lock()
        }
        async fn live_capture(&self) -> Option<&'static str> {
            *self.live.lock()
        }
        fn foreign_slots(&self) -> Vec<String> {
            self.foreign.lock().clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{Fake, at};
    use super::*;

    fn cfg() -> Training {
        Training::default()
    }

    #[tokio::test]
    async fn every_precondition_blocks_and_unblocks() {
        let s = Fake::green();
        assert!(check(Tier::B, &cfg(), &s).await.is_empty());

        *s.now.lock() = at(12, 0);
        assert_eq!(check(Tier::B, &cfg(), &s).await, vec![Blocker::Window]);
        *s.now.lock() = at(3, 0);

        *s.power.lock() = Power::Battery;
        assert_eq!(check(Tier::B, &cfg(), &s).await, vec![Blocker::Power]);
        *s.power.lock() = Power::Unknown;
        assert_eq!(check(Tier::B, &cfg(), &s).await, vec![Blocker::Power]);
        *s.power.lock() = Power::Ac;

        *s.idle.lock() = Some(14.0 * 60.0);
        assert_eq!(check(Tier::B, &cfg(), &s).await, vec![Blocker::Idle]);
        *s.idle.lock() = None;
        assert_eq!(check(Tier::B, &cfg(), &s).await, vec![Blocker::Idle]);
        *s.idle.lock() = Some(15.0 * 60.0);
        assert!(check(Tier::B, &cfg(), &s).await.is_empty());

        *s.live.lock() = Some("meeting");
        assert_eq!(
            check(Tier::B, &cfg(), &s).await,
            vec![Blocker::Live("meeting")]
        );
        *s.live.lock() = None;

        *s.load.lock() = Some(4.0); // cores 8 → limit 4
        assert_eq!(check(Tier::B, &cfg(), &s).await, vec![Blocker::Load]);
        *s.load.lock() = None;
        assert!(check(Tier::B, &cfg(), &s).await.is_empty());
        *s.load.lock() = Some(1.0);

        *s.thermal.lock() = Thermal::Fair;
        assert_eq!(check(Tier::B, &cfg(), &s).await, vec![Blocker::Thermal]);
        *s.thermal.lock() = Thermal::Unknown;
        assert!(check(Tier::B, &cfg(), &s).await.is_empty());
        *s.thermal.lock() = Thermal::Nominal;

        *s.disk.lock() = Some(9 << 30);
        assert_eq!(check(Tier::B, &cfg(), &s).await, vec![Blocker::Disk]);
        *s.disk.lock() = Some(100 << 30);

        *s.memory.lock() = MemoryPressure::Warn;
        assert_eq!(check(Tier::B, &cfg(), &s).await, vec![Blocker::Memory]);
        *s.memory.lock() = MemoryPressure::Normal;

        s.foreign.lock().push("126-W2".into());
        assert_eq!(
            check(Tier::B, &cfg(), &s).await,
            vec![Blocker::BuildSlot("126-W2".into())]
        );
        s.foreign.lock().clear();
        assert!(check(Tier::B, &cfg(), &s).await.is_empty());
    }

    #[tokio::test]
    async fn tier_a_only_yields_to_live_captures_and_foreign_slots() {
        let s = Fake::green();
        *s.now.lock() = at(14, 0);
        *s.power.lock() = Power::Battery;
        *s.idle.lock() = Some(0.0);
        assert!(check(Tier::A, &cfg(), &s).await.is_empty());
        *s.live.lock() = Some("dictation");
        s.foreign.lock().push("126-C".into());
        assert_eq!(
            check(Tier::A, &cfg(), &s).await,
            vec![
                Blocker::Live("dictation"),
                Blocker::BuildSlot("126-C".into())
            ]
        );
    }

    #[test]
    fn window_parses_and_wraps() {
        let w = Window::parse("02:00-06:00").unwrap();
        assert!(w.contains(NaiveTime::from_hms_opt(2, 0, 0).unwrap()));
        assert!(!w.contains(NaiveTime::from_hms_opt(6, 0, 0).unwrap()));
        let w = Window::parse("23:00-05:00").unwrap();
        assert!(w.contains(NaiveTime::from_hms_opt(23, 30, 0).unwrap()));
        assert!(w.contains(NaiveTime::from_hms_opt(1, 0, 0).unwrap()));
        assert!(!w.contains(NaiveTime::from_hms_opt(12, 0, 0).unwrap()));
        let d = chrono::NaiveDate::from_ymd_opt(2026, 10, 7).unwrap();
        assert_eq!(
            w.night_of(d, NaiveTime::from_hms_opt(1, 0, 0).unwrap()),
            d.pred_opt().unwrap()
        );
        assert_eq!(
            w.night_of(d, NaiveTime::from_hms_opt(23, 30, 0).unwrap()),
            d
        );
        assert!(Window::parse("2-6").is_none());
    }

    #[test]
    fn slot_protocol_interoperates_with_the_script() {
        let dir = tempfile::tempdir().unwrap();
        let slots = dir.path();
        // The script's format, written by hand for a live pid (ours): refused.
        let slot0 = slots.join("0");
        std::fs::create_dir_all(&slot0).unwrap();
        std::fs::write(
            slot0.join("owner"),
            format!(
                "{} 126-W2 1759840000 cargo test -p double\n",
                std::process::id()
            ),
        )
        .unwrap();
        assert!(
            BuildSlot::try_acquire(slots, TRAIN_LABEL, "tier a")
                .unwrap()
                .is_none()
        );
        assert_eq!(foreign_slots(slots, TRAIN_LABEL), vec!["126-W2".to_owned()]);
        assert!(foreign_slots(slots, "126-W2").is_empty());

        // A dead pid is stale: reclaimed, and our owner file is byte-compatible.
        std::fs::write(
            slot0.join("owner"),
            "999999999 126-C 1759840000 cargo test -p double\n",
        )
        .unwrap();
        assert!(foreign_slots(slots, TRAIN_LABEL).is_empty());
        let held = BuildSlot::try_acquire(slots, TRAIN_LABEL, "tier b")
            .unwrap()
            .unwrap();
        let owner = Owner::read(held.path()).unwrap();
        assert_eq!(owner.pid, std::process::id());
        assert_eq!(owner.label, TRAIN_LABEL);
        assert_eq!(owner.what, "tier b");
        assert!(owner.epoch > 1_700_000_000);
        let raw = std::fs::read_to_string(held.path().join("owner")).unwrap();
        assert_eq!(raw.split(' ').count(), 5, "{raw:?}"); // pid label epoch "tier" "b"
        assert!(raw.ends_with('\n'));

        // Held by us → a second acquire in this process is refused (we are alive).
        assert!(
            BuildSlot::try_acquire(slots, TRAIN_LABEL, "again")
                .unwrap()
                .is_none()
        );
        drop(held);
        assert!(!slot0.exists());
        assert!(
            BuildSlot::try_acquire(slots, TRAIN_LABEL, "after")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn owner_parses_the_scripts_line() {
        let o = Owner::parse("4242 126-W2 1759840000 cargo test -p double -p platform\n").unwrap();
        assert_eq!(o.pid, 4242);
        assert_eq!(o.label, "126-W2");
        assert_eq!(o.epoch, 1759840000);
        assert_eq!(o.what, "cargo test -p double -p platform");
        assert!(Owner::parse("garbage").is_none());
    }
}
