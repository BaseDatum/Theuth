//! Event router (216 D3, §6.2, §7.3–§7.4): wire feed → `external_view` → remote-session drop →
//! per-plugin grant and subscription match → meeting-session strip → redaction → `string_cap` →
//! each matching plugin's event lane.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use serde_json::{Map, Value, json};
use tokio::sync::broadcast::error::RecvError;
use wire::RuntimeWire as _;

use crate::catalog::DELTA_TYPES;
use crate::config::PluginsConfig;
use crate::manifest::Subscription;
use crate::proc::Outbox;

/// The topic whose text fields are withheld for meeting sessions, and whose
/// `SessionOwnerChanged` tracks remote sessions.
const AGENT_TOPIC: &str = "agent.events";
/// The topic whose `Started` names a meeting session.
const MEETING_TOPIC: &str = "meeting.events";
/// How long the feed waits for the meeting and agent actors when seeding. Plugins start in order
/// after both, so they are registered at once or never (as `secrets::HANDLE_WAIT`).
const SEED_WAIT: Duration = Duration::from_secs(10);
/// How often an unseeded feed tries to seed again (it withholds meanwhile, see [`Scope::seeded`]).
const RESEED_EVERY: Duration = Duration::from_secs(30);

/// 216 §7.3 (b): `agent.events` fields removed for a meeting session and its descendants, by
/// `type`. `TitleChanged`, `StatusChanged` and `ResumeReport.replayed` carry transcript-derived
/// text too (owner ruling on review R24).
const MEETING_STRIP: &[(&str, &str)] = &[
    ("TurnStarted", "text"),
    ("InputQueued", "text"),
    ("TextDelta", "text"),
    ("ReasoningDelta", "text"),
    ("ToolCallStarted", "args"),
    ("ToolCallProgress", "args"),
    ("ToolCallFinished", "output"),
    ("MessageDelivered", "text"),
    ("Error", "message"),
    ("Suggestions", "items"),
    ("TodoChanged", "snapshot"),
    ("AdvisoryAdded", "advisory"),
    ("TitleChanged", "title"),
    ("StatusChanged", "status"),
    ("StatusChanged", "title"),
    ("ResumeReport", "replayed"),
];

/// 216 §7.3 (owner ruling on review R24): `meeting.events#OracleDecision` fields removed for every
/// meeting, under `decision`: verbatim fragments of what was said. `tier2` quotes the subject.
const ORACLE_STRIP: &[&str] = &["mentions", "subject", "tier2"];

/// A running plugin as the router sees it.
#[derive(Clone)]
pub(crate) struct Target {
    /// Plugin name.
    pub name: String,
    /// The manifest's subscriptions.
    pub subs: Vec<Subscription>,
    /// Granted subscription names (`topic`, `topic#Type`, `plugin.event`, `plugin.*`).
    pub granted: Vec<String>,
    /// The plugin's outbox; events go on its event lane.
    pub outbox: Outbox,
}

/// The running plugins, shared by the host (writer) and the feed loop (reader).
pub(crate) type Targets = Arc<RwLock<Vec<Target>>>;

/// Stateful filters: meeting sessions (whole life), remote sessions (SessionOwnerChanged here:false/true).
#[derive(Debug, Default)]
pub(crate) struct Scope {
    /// Sessions of meetings and their descendants: `agent.events` text withheld for the session's
    /// whole life (Q1). A `Deleted` meeting keeps its entry: the frame carries no session, and the
    /// agent session is deleted with the meeting.
    meetings: HashSet<String>,
    /// Sessions last seen owned by another engine.
    remote: HashSet<String>,
    /// Both sets were seeded from the meeting and agent actors and no frame was lost since (review
    /// R6). While `false` the router withholds `agent.events` text for every session; the remote
    /// drop applies only to sessions known to be remote.
    seeded: bool,
}

impl Scope {
    /// Remember a meeting session (seeded from `meeting::ListMeetings`, or a `Started` frame).
    pub(crate) fn add_meeting(&mut self, session: &str) {
        if !self.meetings.contains(session) {
            self.meetings.insert(session.to_owned());
        }
    }

    /// Update the sets from one raw frame (before any exposure projection).
    fn observe(&mut self, topic: &str, event: &Value) {
        let tag = event.get("type").and_then(Value::as_str);
        let session = event.get("session").and_then(Value::as_str);
        let field = |k: &str| event.get(k).and_then(Value::as_str);
        match (topic, tag, session) {
            (MEETING_TOPIC, Some("Started"), Some(s)) => self.add_meeting(s),
            // R24: a child of a meeting session (or of one of its children) is one too.
            (AGENT_TOPIC, Some("SubagentSpawned" | "ObservedChildChanged"), _) => {
                if let (Some(parent), Some(child)) = (field("parent"), field("child"))
                    && self.meetings.contains(parent)
                {
                    self.add_meeting(child);
                }
            }
            (AGENT_TOPIC, Some("SessionOwnerChanged"), Some(s)) => {
                match event.get("here").and_then(Value::as_bool) {
                    Some(false) => {
                        self.remote.insert(s.to_owned());
                    }
                    Some(true) => {
                        self.remote.remove(s);
                    }
                    None => {}
                }
            }
            _ => {}
        }
    }

    /// Seed from the session list: every session owned elsewhere is remote, and every descendant
    /// of a meeting session (already in the set) is a meeting session.
    fn seed_sessions(&mut self, sessions: &[(String, Option<String>, bool)]) {
        for (id, _, here) in sessions {
            if *here {
                self.remote.remove(id);
            } else {
                self.remote.insert(id.clone());
            }
        }
        // Parents may be listed after their children: repeat until nothing is added.
        loop {
            let before = self.meetings.len();
            for (id, parent, _) in sessions {
                if parent.as_ref().is_some_and(|p| self.meetings.contains(p)) {
                    self.add_meeting(id);
                }
            }
            if self.meetings.len() == before {
                break;
            }
        }
    }
}

/// Routes wire frames to plugin event lanes.
pub(crate) struct Router {
    /// Meeting and remote session sets.
    pub scope: Scope,
    cfg: PluginsConfig,
    /// Topic → current version: an empty `version:` list accepts only this one.
    versions: HashMap<String, u32>,
}

/// What a frame is matched on: its catalog name parts.
struct Key {
    topic: &'static str,
    /// Wire topic version (the line's `version`).
    version: u32,
    /// Serialized `type` tag, or `<plugin>.<event>` for plugin events.
    r#type: Option<String>,
    /// Plugin events: the emitting plugin and the event's own version.
    plugin: Option<(String, u32)>,
}

impl Router {
    /// A router with empty session sets.
    pub(crate) fn new(cfg: PluginsConfig, versions: HashMap<String, u32>) -> Self {
        Self {
            scope: Scope::default(),
            cfg,
            versions,
        }
    }

    /// One frame → zero or more `event` lines pushed to target outboxes (order of contract §13 B1):
    /// external_view → remote drop → per-target grant+sub match → meeting strip → redact_value → string_cap → stub if > max_line_bytes.
    /// Redaction runs once per frame and only when some target matches. `now_ms` = at_ms.
    pub(crate) fn deliver(&mut self, frame: &wire::WireFrame, targets: &[Target], now_ms: u64) {
        let Some(descriptor) = frame.descriptor.as_deref() else {
            return;
        };
        self.scope.observe(descriptor.topic, &frame.event);
        let Some(view) = frame.external_view() else {
            return;
        };
        let session = frame.policy.session.as_deref();
        if !self.cfg.remote_sessions && session.is_some_and(|s| self.scope.remote.contains(s)) {
            return;
        }
        let Some(key) = key(descriptor, &view) else {
            return;
        };
        let matched: Vec<&Target> = targets.iter().filter(|t| self.matches(t, &key)).collect();
        if matched.is_empty() {
            return;
        }
        let mut data = view.into_owned();
        if key.plugin.is_some()
            && let Some(map) = data.as_object_mut()
        {
            // §6.2: a plugin event's `data` is the envelope; its line `type` names the event.
            map.remove("type");
        }
        if descriptor.topic == AGENT_TOPIC
            && (!self.scope.seeded || session.is_some_and(|s| self.scope.meetings.contains(s)))
        {
            strip_meeting(&mut data);
        }
        if descriptor.topic == MEETING_TOPIC {
            strip_oracle(&mut data);
        }
        ::tools::secrets::redact_value(&mut data);
        cap_strings(&mut data, self.cfg.string_cap);
        let msg = self.line(&key, session, now_ms, data);
        for target in matched {
            target.outbox.event(msg.clone());
        }
    }

    /// Whether `t` subscribes to the frame and holds a grant for it.
    fn matches(&self, t: &Target, key: &Key) -> bool {
        let granted = |name: &str| t.granted.iter().any(|g| g == name);
        if let (Some((plugin, version)), Some(name)) = (&key.plugin, &key.r#type) {
            return t.subs.iter().any(|sub| {
                let Some(event) = sub.event.as_deref() else {
                    return false;
                };
                let named = event == name || event.strip_suffix(".*") == Some(plugin.as_str());
                named
                    && (sub.version.is_empty() || sub.version.contains(version))
                    && (granted(event) || granted(name))
            });
        }
        let tag = key.r#type.as_deref();
        let delta = tag.is_some_and(|t| DELTA_TYPES.contains(&t));
        let typed_grant = tag.is_some_and(|t| granted(&format!("{}#{t}", key.topic)));
        if !(granted(key.topic) || typed_grant) {
            return false;
        }
        t.subs.iter().any(|sub| {
            sub.topic.as_deref() == Some(key.topic)
                && (sub.types.is_empty() || tag.is_some_and(|t| sub.types.iter().any(|x| x == t)))
                && (!delta || sub.deltas)
                && if sub.version.is_empty() {
                    self.versions
                        .get(key.topic)
                        .is_none_or(|v| *v == key.version)
                } else {
                    sub.version.contains(&key.version)
                }
        })
    }

    /// The `event` notification, or its stub when the line would exceed `max_line_bytes` (§6.1).
    fn line(&self, key: &Key, session: Option<&str>, now_ms: u64, data: Value) -> Value {
        let mut params = Map::new();
        params.insert("topic".into(), key.topic.into());
        params.insert("version".into(), key.version.into());
        if let Some(t) = &key.r#type {
            params.insert("type".into(), t.as_str().into());
        }
        if let Some(s) = session {
            params.insert("session".into(), s.into());
        }
        params.insert("at_ms".into(), now_ms.into());
        params.insert("data".into(), data);
        let msg = json!({"jsonrpc": "2.0", "method": "event", "params": params});
        let bytes = encoded_len(&msg);
        if bytes <= self.cfg.max_line_bytes {
            return msg;
        }
        let mut stub = match msg {
            Value::Object(mut m) => match m.remove("params") {
                Some(Value::Object(p)) => p,
                _ => Map::new(),
            },
            _ => Map::new(),
        };
        stub.remove("data");
        stub.remove("at_ms");
        stub.insert("truncated".into(), true.into());
        stub.insert("bytes".into(), bytes.into());
        json!({"jsonrpc": "2.0", "method": "event", "params": stub})
    }
}

/// The match key of a frame whose external view is `view`; `None` for a `plugins.events` frame
/// without its `{plugin, event, version}` envelope.
fn key(d: &wire::EventDescriptor, view: &Value) -> Option<Key> {
    let field = |k: &str| view.get(k).and_then(Value::as_str);
    if d.topic != crate::EVENTS_TOPIC {
        let tag = field("type").map(str::to_owned);
        return Some(Key {
            topic: d.topic,
            version: d.version,
            r#type: tag,
            plugin: None,
        });
    }
    let (plugin, event) = (field("plugin")?, field("event")?);
    let version = view
        .get("version")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())?;
    Some(Key {
        topic: d.topic,
        version: d.version,
        r#type: Some(format!("{plugin}.{event}")),
        plugin: Some((plugin.to_owned(), version)),
    })
}

/// Remove the §7.3 meeting-session fields of one `agent.events` frame.
fn strip_meeting(data: &mut Value) {
    let tag = data.get("type").and_then(Value::as_str).map(str::to_owned);
    let Some(map) = data.as_object_mut() else {
        return;
    };
    for (t, field) in MEETING_STRIP {
        if tag.as_deref() == Some(*t) {
            map.remove(*field);
        }
    }
}

/// Remove [`ORACLE_STRIP`] from a `meeting.events#OracleDecision` frame's `decision`.
fn strip_oracle(data: &mut Value) {
    if data.get("type").and_then(Value::as_str) != Some("OracleDecision") {
        return;
    }
    if let Some(decision) = data.get_mut("decision").and_then(Value::as_object_mut) {
        for field in ORACLE_STRIP {
            decision.remove(*field);
        }
    }
}

/// Cut every string longer than `cap` bytes to `cap` (at a char boundary) + `…[+N bytes]`.
fn cap_strings(v: &mut Value, cap: usize) {
    match v {
        Value::String(s) if s.len() > cap => {
            let mut end = cap;
            while !s.is_char_boundary(end) {
                end -= 1;
            }
            let cut = s.len() - end;
            s.truncate(end);
            s.push_str(&format!("…[+{cut} bytes]"));
        }
        Value::Array(items) => items.iter_mut().for_each(|i| cap_strings(i, cap)),
        Value::Object(map) => map.values_mut().for_each(|i| cap_strings(i, cap)),
        _ => {}
    }
}

/// Serialized length of `v` without allocating the line.
fn encoded_len(v: &Value) -> usize {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 += buf.len();
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    // Writing a `Value` into a sink that never fails cannot fail.
    let _ = serde_json::to_writer(&mut count, v);
    count.0
}

/// Wall-clock milliseconds (the feed has no timestamp, D3).
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Subscribe to rt.wire_events() FIRST, then seed (meetings, then sessions), then loop until
/// rt.shutdown. Lagged(n) → every target.outbox.lagged(n), and the sets are re-seeded before the
/// next frame is routed (frames lost may have named a meeting or a move). Until a seed succeeds
/// the router fails closed ([`Scope::seeded`]) and retries every [`RESEED_EVERY`].
pub(crate) async fn feed_loop(rt: runtime::Runtime, targets: Targets, cfg: PluginsConfig) {
    let mut rx = rt.wire_events();
    let versions = rt
        .wire_schema()
        .iter()
        .map(|d| (d.topic.to_owned(), d.version))
        .collect();
    let mut router = Router::new(cfg, versions);
    seed(&rt, &mut router.scope, SEED_WAIT).await;
    let mut retry = tokio::time::interval(RESEED_EVERY);
    retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let item = tokio::select! {
            () = rt.shutdown.cancelled() => break,
            _ = retry.tick(), if !router.scope.seeded => {
                seed(&rt, &mut router.scope, Duration::ZERO).await;
                continue;
            }
            item = rx.recv() => item,
        };
        match item {
            Ok(frame) => {
                let targets = targets.read().unwrap_or_else(PoisonError::into_inner);
                router.deliver(&frame, &targets, now_ms());
            }
            Err(RecvError::Lagged(n)) => {
                tracing::warn!(
                    dropped = n,
                    "plugins: wire feed lagged; re-seeding session filters"
                );
                for t in targets
                    .read()
                    .unwrap_or_else(PoisonError::into_inner)
                    .iter()
                {
                    t.outbox.lagged(n);
                }
                router.scope.seeded = false;
                seed(&rt, &mut router.scope, Duration::ZERO).await;
            }
            Err(RecvError::Closed) => break,
        }
    }
}

/// Seed both sets; [`Scope::seeded`] only when both sources answered. `wait` bounds the wait for
/// each actor to register: [`SEED_WAIT`] at start, zero on a retry or after a lag (the feed
/// must not stall), and only the first attempt warns.
async fn seed(rt: &runtime::Runtime, scope: &mut Scope, wait: Duration) {
    let meetings = seed_meetings(rt, scope, wait).await;
    let sessions = meetings && seed_sessions(rt, scope, wait).await;
    scope.seeded = sessions;
    if !sessions && !wait.is_zero() {
        tracing::warn!(
            "plugins: session filters not seeded; agent.events text withheld until they are"
        );
    }
}

/// Every meeting's session into the meeting set, from the meeting actor. `true` when meetings
/// are off or the list arrived.
async fn seed_meetings(rt: &runtime::Runtime, scope: &mut Scope, wait: Duration) -> bool {
    let cfg: meeting::MeetingConfig = rt.section("meeting").unwrap_or_default();
    if !cfg.enabled {
        return true;
    }
    let lookup = rt.lookup_when::<meeting::Meeting>(meeting::MEETING_ACTOR);
    let Ok(Some(actor)) = tokio::time::timeout(wait, lookup).await else {
        tracing::debug!("plugins: meeting actor not found; meeting sessions not seeded");
        return false;
    };
    match actor
        .ask(meeting::ListMeetings {
            limit: Some(u32::MAX),
            project: None,
        })
        .await
    {
        Ok(list) => {
            for m in list {
                scope.add_meeting(&m.session.to_string());
            }
            true
        }
        Err(e) => {
            tracing::warn!(error = %e, "plugins: ListMeetings failed; meeting sessions not seeded");
            false
        }
    }
}

/// Remote sessions and meeting descendants from the agent supervisor's session list.
async fn seed_sessions(rt: &runtime::Runtime, scope: &mut Scope, wait: Duration) -> bool {
    let lookup = rt.lookup_when::<agent::Agents>(agent::AGENT_ACTOR);
    let Ok(Some(agents)) = tokio::time::timeout(wait, lookup).await else {
        tracing::debug!("plugins: agent supervisor not found; sessions not seeded");
        return false;
    };
    match agents.ask(agent::ListSessions).await {
        Ok(list) => {
            let rows: Vec<(String, Option<String>, bool)> = list
                .iter()
                .map(|s| (s.id.to_string(), s.parent.map(|p| p.to_string()), s.here))
                .collect();
            scope.seed_sessions(&rows);
            true
        }
        Err(e) => {
            tracing::warn!(error = %e, "plugins: ListSessions failed; sessions not seeded");
            false
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use tokio::io::AsyncBufReadExt as _;
    use wire::{EventDescriptor, EventPolicy, WireFrame};

    const S: &str = "01a20e56-84d4-775f-ae0c-65af2ba8c55b";
    const T: &str = "01a20e56-84d4-775f-ae0c-65af2ba8c55c";

    /// Probe events of the route tests.
    #[expect(dead_code, reason = "only its descriptor is used; frames are JSON")]
    #[derive(Clone, serde::Serialize, schemars::JsonSchema)]
    #[serde(tag = "type")]
    enum Probe {
        /// Something said.
        Said {
            /// Session.
            session: String,
            /// Text.
            text: String,
        },
        /// A delta.
        TextDelta {
            /// Session.
            session: String,
        },
        /// Another thing.
        Other {
            /// A number.
            n: u32,
        },
    }

    impl wire::WireEvent for Probe {
        const TOPIC: &'static str = "test.route-probe";
    }

    /// A private topic.
    #[derive(Clone, serde::Serialize, schemars::JsonSchema)]
    struct Hidden {
        /// Private.
        sites: Vec<String>,
    }

    impl wire::WireEvent for Hidden {
        const TOPIC: &'static str = "test.route-hidden";
        const EXPOSURE: wire::Exposure = wire::Exposure::Private;
    }

    fn frame(d: EventDescriptor, event: Value) -> Arc<WireFrame> {
        Arc::new(WireFrame {
            descriptor: Some(Arc::new(d)),
            policy: EventPolicy::legacy_fields(&event),
            event,
        })
    }

    fn probe(event: Value) -> Arc<WireFrame> {
        frame(EventDescriptor::of::<Probe>(), event)
    }

    fn agent(event: Value) -> Arc<WireFrame> {
        frame(EventDescriptor::of::<agent::AgentEvent>(), event)
    }

    fn said(text: &str) -> Value {
        json!({"type": "Said", "session": S, "text": text})
    }

    fn topic_sub(topic: &str, types: &[&str]) -> Subscription {
        Subscription {
            topic: Some(topic.into()),
            event: None,
            types: types.iter().map(|t| (*t).to_owned()).collect(),
            version: Vec::new(),
            deltas: false,
        }
    }

    fn event_sub(event: &str) -> Subscription {
        Subscription {
            topic: None,
            event: Some(event.into()),
            types: Vec::new(),
            version: Vec::new(),
            deltas: false,
        }
    }

    fn target(subs: Vec<Subscription>, granted: &[&str], queue: usize) -> Target {
        Target {
            name: "p".into(),
            subs,
            granted: granted.iter().map(|g| (*g).to_owned()).collect(),
            outbox: Outbox::new(queue),
        }
    }

    fn router(cfg: PluginsConfig) -> Router {
        let versions = [
            ("test.route-probe", 1),
            ("agent.events", 1),
            ("meeting.events", 1),
            ("plugins.events", 1),
        ];
        let mut r = Router::new(
            cfg,
            versions
                .into_iter()
                .map(|(t, v)| (t.to_owned(), v))
                .collect(),
        );
        r.scope.seeded = true;
        r
    }

    /// Push a sentinel event, then read lines until it: everything delivered before it, in order.
    async fn delivered(outbox: &Outbox) -> Vec<Value> {
        outbox.event(json!({"sentinel": true}));
        let (w, r) = tokio::io::duplex(1 << 22);
        tokio::spawn(crate::proc::write_loop(outbox.clone(), w));
        let mut lines = tokio::io::BufReader::new(r).lines();
        let mut out = Vec::new();
        loop {
            let line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
                .await
                .expect("line within 5 s")
                .expect("read")
                .expect("sentinel before EOF");
            let v: Value = serde_json::from_str(&line).expect("json line");
            if v == json!({"sentinel": true}) {
                break;
            }
            out.push(v);
        }
        outbox.close();
        out
    }

    /// The vault's redaction hook for tests: one known value.
    pub(crate) struct Fake;

    pub(crate) fn placeholder() -> String {
        ["{", "{", "secret:PASS", "}", "}"].concat()
    }

    impl ::tools::secrets::Secrets for Fake {
        fn expand(&self, _: &mut Value) -> Result<usize, String> {
            Ok(0)
        }

        fn redact<'a>(&self, text: &'a str) -> std::borrow::Cow<'a, str> {
            if text.contains("hunter22") {
                std::borrow::Cow::Owned(text.replace("hunter22", &placeholder()))
            } else {
                std::borrow::Cow::Borrowed(text)
            }
        }

        fn redact_value(&self, v: &mut Value) {
            match v {
                Value::String(s) => {
                    if let std::borrow::Cow::Owned(clean) = self.redact(s) {
                        *s = clean;
                    }
                }
                Value::Array(items) => items.iter_mut().for_each(|i| self.redact_value(i)),
                Value::Object(map) => map.values_mut().for_each(|i| self.redact_value(i)),
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn vault_value_reaches_plugin_redacted() {
        ::tools::secrets::install(Arc::new(Fake));
        let cfg = PluginsConfig {
            string_cap: 32,
            ..PluginsConfig::default()
        };
        let mut r = router(cfg);
        let t = target(
            vec![topic_sub("test.route-probe", &[])],
            &["test.route-probe"],
            16,
        );
        // Redaction runs before the cap: the secret sits across byte 32 of the raw text.
        let raw = format!("{}hunter22 tail", "a".repeat(28));
        r.deliver(&probe(said(&raw)), std::slice::from_ref(&t), 7);
        r.deliver(&probe(said("short hunter22")), std::slice::from_ref(&t), 8);
        let lines = delivered(&t.outbox).await;
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(
            !lines.iter().any(|l| l.to_string().contains("hunter22")),
            "{lines:?}"
        );
        let full = format!("{}{} tail", "a".repeat(28), placeholder());
        let kept = &full[..32];
        assert_eq!(
            lines[0]["params"]["data"]["text"],
            json!(format!("{kept}…[+{} bytes]", full.len() - 32))
        );
        assert_eq!(
            lines[1]["params"]["data"]["text"],
            json!(format!("short {}", placeholder()))
        );
        let p = &lines[1]["params"];
        assert_eq!(lines[1]["jsonrpc"], "2.0");
        assert_eq!(lines[1]["method"], "event");
        assert_eq!(
            (
                &p["topic"],
                &p["version"],
                &p["type"],
                &p["session"],
                &p["at_ms"]
            ),
            (
                &json!("test.route-probe"),
                &json!(1),
                &json!("Said"),
                &json!(S),
                &json!(8)
            )
        );

        // Over max_line_bytes after the cap: a stub without data.
        let cfg = PluginsConfig {
            max_line_bytes: 200,
            ..PluginsConfig::default()
        };
        let mut r = router(cfg);
        let t2 = target(
            vec![topic_sub("test.route-probe", &[])],
            &["test.route-probe"],
            16,
        );
        r.deliver(&probe(said(&"z".repeat(500))), std::slice::from_ref(&t2), 9);
        let lines = delivered(&t2.outbox).await;
        let p = &lines[0]["params"];
        assert_eq!(p["truncated"], json!(true), "{p}");
        assert!(p["bytes"].as_u64().is_some_and(|b| b > 500), "{p}");
        assert!(p.get("data").is_none(), "{p}");
        assert_eq!(
            (&p["topic"], &p["type"], &p["session"]),
            (&json!("test.route-probe"), &json!("Said"), &json!(S))
        );
    }

    #[tokio::test]
    async fn meeting_session_text_stripped_for_life() {
        let mut r = router(PluginsConfig::default());
        let mut sub = topic_sub("agent.events", &[]);
        sub.deltas = true;
        let t = target(vec![sub], &["agent.events"], 64);
        let targets = std::slice::from_ref(&t);
        let meeting = frame(
            EventDescriptor::of::<meeting::MeetingEvent>(),
            json!({"type": "Started", "meeting": "01a2", "session": S, "process": {"pid": 0, "name": "x", "bundle_id": null, "active": true}, "at": "2026-11-25T10:00:00Z"}),
        );
        r.deliver(&meeting, &[], 1);
        // A seeded meeting session (ListMeetings at start).
        let seeded = "01a20e56-84d4-775f-ae0c-65af2ba8c5ff";
        r.scope.add_meeting(seeded);
        let turn = |session: &str, text: &str| json!({"type": "TurnStarted", "session": session, "turn": "01a3", "text": text, "model": "m", "turn_seq": 1});
        r.deliver(&agent(turn(S, "what they said")), targets, 2);
        r.deliver(
            &agent(json!({"type": "TextDelta", "session": S, "turn": "01a3", "text": "words"})),
            targets,
            3,
        );
        r.deliver(&agent(json!({"type": "ToolCallFinished", "session": S, "turn": "01a3", "call_id": "c", "output": "out", "is_error": false, "files": []})), targets, 4);
        r.deliver(&agent(turn(T, "owner words")), targets, 5);
        r.deliver(&agent(turn(seeded, "seeded words")), targets, 6);
        // After the meeting is deleted, the session stays withheld.
        let deleted = frame(
            EventDescriptor::of::<meeting::MeetingEvent>(),
            json!({"type": "Deleted", "meeting": "01a2"}),
        );
        r.deliver(&deleted, targets, 7);
        r.deliver(&agent(json!({"type": "InputQueued", "session": S, "id": "i", "text": "later", "defer": false})), targets, 8);
        let lines = delivered(&t.outbox).await;
        let data: Vec<&Value> = lines.iter().map(|l| &l["params"]["data"]).collect();
        assert_eq!(data.len(), 6, "{lines:?}");
        assert_eq!(data[0]["type"], "TurnStarted");
        assert!(data[0].get("text").is_none(), "{}", data[0]);
        assert_eq!(data[0]["turn"], "01a3");
        assert_eq!(data[0]["model"], "m");
        assert!(data[1].get("text").is_none(), "{}", data[1]);
        assert!(data[2].get("output").is_none(), "{}", data[2]);
        assert_eq!(data[2]["is_error"], false);
        assert_eq!(data[3]["text"], "owner words");
        assert!(data[4].get("text").is_none(), "{}", data[4]);
        assert_eq!(data[5]["type"], "InputQueued");
        assert!(data[5].get("text").is_none(), "{}", data[5]);
        assert!(
            !lines
                .iter()
                .any(|l| l.to_string().contains("what they said"))
        );
    }

    #[tokio::test]
    async fn remote_session_dropped_unless_enabled() {
        let owner = |here: bool| json!({"type": "SessionOwnerChanged", "session": S, "owner": "e2", "epoch": 2, "here": here});
        let turn = |text: &str| json!({"type": "TurnStarted", "session": S, "turn": "01a3", "text": text, "model": "m", "turn_seq": 1});
        let other = json!({"type": "TurnStarted", "session": T, "turn": "01a4", "text": "local", "model": "m", "turn_seq": 1});

        let mut r = router(PluginsConfig::default());
        let t = target(vec![topic_sub("agent.events", &[])], &["agent.events"], 64);
        let targets = std::slice::from_ref(&t);
        r.deliver(&agent(owner(false)), targets, 1);
        r.deliver(&agent(turn("remote")), targets, 2);
        r.deliver(&agent(other.clone()), targets, 3);
        r.deliver(&agent(owner(true)), targets, 4);
        r.deliver(&agent(turn("back")), targets, 5);
        let lines = delivered(&t.outbox).await;
        let got: Vec<(&Value, &Value)> = lines
            .iter()
            .map(|l| (&l["params"]["type"], &l["params"]["data"]["text"]))
            .collect();
        assert_eq!(
            got,
            vec![
                (&json!("TurnStarted"), &json!("local")),
                (&json!("SessionOwnerChanged"), &Value::Null),
                (&json!("TurnStarted"), &json!("back")),
            ]
        );

        let mut r = router(PluginsConfig {
            remote_sessions: true,
            ..PluginsConfig::default()
        });
        let t = target(
            vec![topic_sub("agent.events", &["TurnStarted"])],
            &["agent.events#TurnStarted"],
            64,
        );
        let targets = std::slice::from_ref(&t);
        r.deliver(&agent(owner(false)), targets, 1);
        r.deliver(&agent(turn("remote")), targets, 2);
        let lines = delivered(&t.outbox).await;
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(lines[0]["params"]["data"]["text"], "remote");
    }

    #[tokio::test]
    async fn overflow_queues_one_lagged() {
        let mut r = router(PluginsConfig::default());
        let t = target(
            vec![topic_sub("test.route-probe", &["Other"])],
            &["test.route-probe#Other"],
            3,
        );
        for n in 0..6 {
            r.deliver(
                &probe(json!({"type": "Other", "n": n})),
                std::slice::from_ref(&t),
                n,
            );
        }
        // The sentinel itself pushes out one more: 6 events + sentinel into 3 slots.
        let lines = delivered(&t.outbox).await;
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert_eq!(lines[0]["method"], "lagged");
        assert_eq!(lines[0]["params"]["dropped"], 4);
        let ns: Vec<&Value> = lines[1..]
            .iter()
            .map(|l| &l["params"]["data"]["n"])
            .collect();
        assert_eq!(ns, vec![&json!(4), &json!(5)]);
    }

    #[tokio::test]
    async fn descriptorless_frame_dropped() {
        let mut r = router(PluginsConfig::default());
        let t = target(
            vec![
                topic_sub("test.route-probe", &[]),
                topic_sub("test.route-hidden", &[]),
            ],
            &["test.route-probe", "test.route-hidden"],
            16,
        );
        let targets = std::slice::from_ref(&t);
        let bare = said("no contract");
        let legacy = Arc::new(WireFrame {
            descriptor: None,
            policy: EventPolicy::legacy_fields(&bare),
            event: bare,
        });
        r.deliver(&legacy, targets, 1);
        r.deliver(
            &frame(EventDescriptor::of::<Hidden>(), json!({"sites": ["a"]})),
            targets,
            2,
        );
        r.deliver(&probe(said("described")), targets, 3);
        let lines = delivered(&t.outbox).await;
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(lines[0]["params"]["data"]["text"], "described");
    }

    #[tokio::test]
    async fn ungranted_topic_not_delivered() {
        let mut r = router(PluginsConfig::default());
        // Subscribes to the whole topic, granted one type only.
        let narrow = target(
            vec![topic_sub("test.route-probe", &[])],
            &["test.route-probe#Said"],
            16,
        );
        // Subscribes, granted nothing.
        let none = target(vec![topic_sub("test.route-probe", &[])], &[], 16);
        // Granted the topic but not subscribed to deltas; and wants only version 2.
        let mut v2 = topic_sub("test.route-probe", &[]);
        v2.version = vec![2];
        let wide = target(
            vec![topic_sub("test.route-probe", &[]), v2],
            &["test.route-probe"],
            16,
        );
        // Plugin events: by `<plugin>.*` (granted), by name (not granted).
        let star = target(vec![event_sub("turns.*")], &["turns.*"], 16);
        let named = target(vec![event_sub("turns.limit_reached")], &[], 16);
        let targets = [
            narrow.clone(),
            none.clone(),
            wide.clone(),
            star.clone(),
            named.clone(),
        ];

        r.deliver(&probe(said("hi")), &targets, 1);
        r.deliver(&probe(json!({"type": "Other", "n": 1})), &targets, 2);
        r.deliver(
            &probe(json!({"type": "TextDelta", "session": S})),
            &targets,
            3,
        );
        let emitted = json!({"type": "PluginEvent", "plugin": "turns", "event": "limit_reached", "version": 1, "session": null, "data": {"turns": 50}});
        r.deliver(
            &frame(EventDescriptor::of::<crate::events::PluginEvent>(), emitted),
            &targets,
            4,
        );

        let types = |lines: &[Value]| -> Vec<String> {
            lines
                .iter()
                .map(|l| l["params"]["type"].as_str().unwrap_or_default().to_owned())
                .collect()
        };
        assert_eq!(types(&delivered(&narrow.outbox).await), vec!["Said"]);
        assert!(delivered(&none.outbox).await.is_empty());
        assert_eq!(types(&delivered(&wide.outbox).await), vec!["Said", "Other"]);
        let lines = delivered(&star.outbox).await;
        assert_eq!(types(&lines), vec!["turns.limit_reached"]);
        assert_eq!(lines[0]["params"]["topic"], "plugins.events");
        assert_eq!(
            lines[0]["params"]["data"],
            json!({"plugin": "turns", "event": "limit_reached", "version": 1, "session": null, "data": {"turns": 50}})
        );
        assert!(delivered(&named.outbox).await.is_empty());
    }

    /// R6: until the sets are seeded (boot failure, or after a lag) `agent.events` text is
    /// withheld for every session; frames still flow.
    #[tokio::test]
    async fn unseeded_scope_withholds_text() {
        let turn = |s: &str| json!({"type": "TurnStarted", "session": s, "turn": "01a3", "text": "words", "model": "m", "turn_seq": 1});
        let deleted = |s: &str| json!({"type": "SessionDeleted", "session": s});
        let mut sub = topic_sub("agent.events", &[]);
        sub.deltas = true;

        let mut r = router(PluginsConfig::default());
        r.scope.seeded = false;
        let t = target(vec![sub], &["agent.events"], 64);
        r.deliver(&agent(turn(S)), std::slice::from_ref(&t), 1);
        r.deliver(&agent(deleted(T)), std::slice::from_ref(&t), 2);
        let lines = delivered(&t.outbox).await;
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(
            lines[0]["params"]["data"].get("text").is_none(),
            "{lines:?}"
        );
        assert_eq!(lines[1]["params"]["data"]["type"], "SessionDeleted");

        // Seeding from the session list: a session owned elsewhere is remote, a meeting's
        // descendants (listed before their parent) are meeting sessions.
        let mut scope = Scope::default();
        scope.add_meeting("m");
        scope.seed_sessions(&[
            ("grandchild".into(), Some("child".into()), true),
            ("child".into(), Some("m".into()), true),
            ("away".into(), None, false),
            ("m".into(), None, true),
        ]);
        assert!(scope.meetings.contains("grandchild") && scope.meetings.contains("child"));
        assert!(scope.remote.contains("away") && !scope.remote.contains("m"));
    }

    /// R24: a meeting session's children are withheld too, `TitleChanged`/`StatusChanged`/
    /// `ResumeReport.replayed` are stripped, and `OracleDecision` loses its verbatim fragments
    /// for every meeting.
    #[tokio::test]
    async fn meeting_children_and_oracle_fragments_stripped() {
        let mut r = router(PluginsConfig::default());
        let t = target(
            vec![
                topic_sub("agent.events", &[]),
                topic_sub("meeting.events", &[]),
            ],
            &["agent.events", "meeting.events"],
            64,
        );
        let targets = std::slice::from_ref(&t);
        r.scope.add_meeting(S);
        r.deliver(
            &agent(json!({"type": "SubagentSpawned", "parent": S, "child": T, "name": "c"})),
            targets,
            1,
        );
        r.deliver(
            &agent(json!({"type": "TitleChanged", "session": T, "title": "about Kuiper"})),
            targets,
            2,
        );
        r.deliver(
            &agent(json!({"type": "StatusChanged", "session": S, "status": "reading"})),
            targets,
            3,
        );
        r.deliver(&agent(json!({"type": "ResumeReport", "session": S, "turn": "t", "replayed": [["c", "x"]], "interrupted": [], "lost_jobs": [], "children_restored": 0, "children_handed_off": 0})), targets, 4);
        let other = "01a20e56-84d4-775f-ae0c-65af2ba8c5aa";
        r.deliver(
            &agent(json!({"type": "TitleChanged", "session": other, "title": "mine"})),
            targets,
            5,
        );
        let oracle = frame(
            EventDescriptor::of::<meeting::MeetingEvent>(),
            json!({"type": "OracleDecision", "meeting": "01a2", "decision": {"batch_id": "10:00:00", "family": "meeting.kind", "mode": "auto", "disposition": "skip", "stages": [], "mentions": ["Kuiper belt"], "subject": "Planet Nine", "tier2": "\"Planet Nine\" → no cached brief", "top": [], "latency_ms": 5, "tier": 1, "outcome": null}}),
        );
        r.deliver(&oracle, targets, 6);
        let lines = delivered(&t.outbox).await;
        let data: Vec<&Value> = lines.iter().map(|l| &l["params"]["data"]).collect();
        let of = |ty: &str| {
            data.iter()
                .filter(|d| d["type"] == ty)
                .copied()
                .collect::<Vec<_>>()
        };
        let titles = of("TitleChanged");
        assert_eq!(titles.len(), 2, "{lines:?}");
        assert!(
            titles[0].get("title").is_none(),
            "child of a meeting: {}",
            titles[0]
        );
        assert_eq!(titles[1]["title"], "mine");
        assert!(of("StatusChanged")[0].get("status").is_none());
        let resume = of("ResumeReport");
        assert!(resume[0].get("replayed").is_none());
        assert_eq!(resume[0]["interrupted"], json!([]));
        let decision = &of("OracleDecision")[0]["decision"];
        assert_eq!(decision["batch_id"], "10:00:00", "{decision}");
        for gone in ["mentions", "subject", "tier2"] {
            assert!(decision.get(gone).is_none(), "{gone}: {decision}");
        }
        assert!(!lines.iter().any(|l| l.to_string().contains("Kuiper")));
    }
}
