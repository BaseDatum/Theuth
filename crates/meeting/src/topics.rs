//! Topic and brief bookkeeping (02 §5, 53 §4.6): the actor is the referee.
//! Pure over `Instant`s — validation, insert with expire-oldest at `max`,
//! substring re-touch, TTL expiry, and the `briefs.jsonl` record shape. No
//! events, no files: the actor publishes and appends what this returns.

use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::cache::CachedBrief;
use crate::{Brief, BriefId, BriefSource, Topic, TopicGone, TopicId, session};

/// Longest label, in words (53 §3.3).
pub const MAX_LABEL_WORDS: usize = 4;
/// Bullets a brief may have.
pub const BULLETS: std::ops::RangeInclusive<usize> = 2..=3;

/// A live topic button.
#[derive(Debug)]
struct Live {
    topic: Topic,
    /// Last time its label was spoken (or it was created).
    touched: Instant,
    /// `updated_at` of the cache row behind it (05 §3: the batch line shows its age).
    cached_at: Option<DateTime<Utc>>,
}

/// One `briefs.jsonl` line (02 §7).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Record {
    /// A topic button appeared.
    Topic(Topic),
    /// A brief was stored (shown or precomputed).
    Brief(Brief),
    /// A topic button went away.
    Removed {
        /// Which.
        topic: TopicId,
        /// Why.
        why: TopicGone,
        /// When, on the recording clock.
        at_ms: u64,
    },
}

/// The live set, the dismissed labels and every brief of one meeting.
#[derive(Debug)]
pub struct Topics {
    live: Vec<Live>,
    dismissed: Vec<String>,
    briefs: Vec<Brief>,
    /// Cache rows shown as a topic this meeting (05 §3): never re-added by the alias scan.
    cached_shown: Vec<BriefId>,
    /// Cache rows whose topic the user dismissed this meeting.
    cached_dismissed: Vec<BriefId>,
    ttl: Duration,
    max: usize,
}

/// A topic that fell off the live set, with its reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gone {
    /// Which.
    pub topic: TopicId,
    /// Why.
    pub why: TopicGone,
}

fn same(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// `label` fits the button: 1–4 words.
pub fn check_label(label: &str) -> Result<(), String> {
    let n = label.split_whitespace().count();
    if n == 0 {
        Err("label is empty".into())
    } else if n > MAX_LABEL_WORDS {
        Err(format!("label has {n} words; at most {MAX_LABEL_WORDS}"))
    } else {
        Ok(())
    }
}

/// `bullets` fit a brief: 2–3, none blank.
pub fn check_bullets(bullets: &[String]) -> Result<(), String> {
    if !BULLETS.contains(&bullets.len()) {
        return Err(format!("{} bullets; a brief has 2 or 3", bullets.len()));
    }
    if bullets.iter().any(|b| b.trim().is_empty()) {
        return Err("a bullet is empty".into());
    }
    Ok(())
}

impl Topics {
    /// Empty; `ttl` and `max` from config.
    pub fn new(ttl_secs: u32, max: u32) -> Self {
        Self {
            live: Vec::new(),
            dismissed: Vec::new(),
            briefs: Vec::new(),
            cached_shown: Vec::new(),
            cached_dismissed: Vec::new(),
            ttl: Duration::from_secs(u64::from(ttl_secs)),
            max: (max as usize).max(1),
        }
    }

    /// Live labels for the batch line, oldest first; a cached topic reads
    /// `<label> (cached <age>)` (05 §3).
    pub fn batch_labels(&self, now: DateTime<Utc>) -> Vec<String> {
        self.live
            .iter()
            .map(|l| match l.cached_at {
                Some(at) => session::cached_label(&l.topic.label, &session::age(at, now)),
                None => l.topic.label.clone(),
            })
            .collect()
    }

    /// The alias scan must skip `row`: already shown or dismissed this meeting.
    pub fn cached_blocked(&self, row: BriefId) -> bool {
        self.cached_shown.contains(&row) || self.cached_dismissed.contains(&row)
    }

    /// A cache row changed (`brief_put`): every instance of it this meeting takes the new
    /// title and bullets; the live topic showing it, if any, is returned so the actor can
    /// re-publish (05 §7 check 8). `None` when nothing here came from `row`.
    pub fn refresh_cached(&mut self, row: &CachedBrief) -> Option<Option<TopicId>> {
        let mut any = false;
        for b in self.briefs.iter_mut().filter(|b| b.cached == Some(row.id)) {
            b.title.clone_from(&row.title);
            b.bullets.clone_from(&row.bullets);
            any = true;
        }
        if !any {
            return None;
        }
        let at = DateTime::parse_from_rfc3339(&row.updated_at).ok().map(|d| d.with_timezone(&Utc));
        let mut topic = None;
        for l in self.live.iter_mut().filter(|l| l.topic.cached == Some(row.id)) {
            l.cached_at = at;
            topic = Some(l.topic.id);
        }
        Some(topic)
    }

    /// Live labels, oldest first.
    pub fn live_labels(&self) -> Vec<String> {
        self.live.iter().map(|l| l.topic.label.clone()).collect()
    }

    /// Dismissed labels, in order.
    pub fn dismissed_labels(&self) -> &[String] {
        &self.dismissed
    }

    /// Live topics, oldest first.
    pub fn live(&self) -> Vec<Topic> {
        self.live.iter().map(|l| l.topic.clone()).collect()
    }

    /// Every brief stored this meeting, in order.
    pub fn briefs(&self) -> &[Brief] {
        &self.briefs
    }

    /// A brief by id.
    pub fn brief(&self, id: BriefId) -> Option<&Brief> {
        self.briefs.iter().find(|b| b.id == id)
    }

    /// The live topic `id`, if it is live.
    pub fn topic(&self, id: TopicId) -> Option<&Topic> {
        self.live.iter().find(|l| l.topic.id == id).map(|l| &l.topic)
    }

    /// `label` may become a topic: 1–4 words, not live, not dismissed.
    pub fn admit(&self, label: &str) -> Result<(), String> {
        check_label(label)?;
        if self.live.iter().any(|l| same(&l.topic.label, label)) {
            return Err("topic already live".into());
        }
        if self.dismissed.iter().any(|d| same(d, label)) {
            return Err("topic already dismissed".into());
        }
        Ok(())
    }

    /// `meeting_topic`: validate, make room, insert the topic with its
    /// precomputed brief (`cached` = the cache row it was written to, 05 §4).
    /// `Ok((topic, brief, expired))`; `expired` are the oldest-untouched
    /// topics that made room (`TopicGone::Expired`).
    pub fn insert(
        &mut self,
        label: &str,
        title: &str,
        bullets: Vec<String>,
        cached: Option<BriefId>,
        now: Instant,
        at_ms: u64,
    ) -> Result<(Topic, Brief, Vec<Gone>), String> {
        self.admit(label)?;
        check_bullets(&bullets)?;
        Ok(self.place(label, title, bullets, BriefSource::Auto, cached, None, now, at_ms))
    }

    /// The alias scan hit `row`, or the model registered it by reference
    /// (05 §3–§4): same rules as [`Self::insert`], plus the row must not have
    /// been shown or dismissed this meeting; the brief is the row's, `source:
    /// Cache`. `label` is the alias as spoken.
    pub fn insert_cached(
        &mut self,
        label: &str,
        row: &CachedBrief,
        now: Instant,
        at_ms: u64,
    ) -> Result<(Topic, Brief, Vec<Gone>), String> {
        self.admit(label)?;
        if self.cached_blocked(row.id) {
            return Err("cached brief already shown or dismissed this meeting".into());
        }
        let at = DateTime::parse_from_rfc3339(&row.updated_at).ok().map(|d| d.with_timezone(&Utc));
        Ok(self.place(
            label,
            &row.title,
            row.bullets.clone(),
            BriefSource::Cache,
            Some(row.id),
            at,
            now,
            at_ms,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn place(
        &mut self,
        label: &str,
        title: &str,
        bullets: Vec<String>,
        source: BriefSource,
        cached: Option<BriefId>,
        cached_at: Option<DateTime<Utc>>,
        now: Instant,
        at_ms: u64,
    ) -> (Topic, Brief, Vec<Gone>) {
        let mut expired = Vec::new();
        while self.live.len() >= self.max {
            let oldest = self
                .live
                .iter()
                .enumerate()
                .min_by_key(|(_, l)| l.touched)
                .map(|(i, _)| i)
                .expect("non-empty");
            let l = self.live.remove(oldest);
            expired.push(Gone {
                topic: l.topic.id,
                why: TopicGone::Expired,
            });
        }
        let topic = Topic {
            id: TopicId::new(),
            label: label.split_whitespace().collect::<Vec<_>>().join(" "),
            at_ms,
            brief: Some(BriefId::new()),
            cached,
        };
        let brief = Brief {
            id: topic.brief.expect("set"),
            topic: Some(topic.id),
            title: title.trim().to_owned(),
            bullets: bullets.into_iter().map(|b| b.trim().to_owned()).collect(),
            source,
            at_ms,
            parent: None,
            cached,
        };
        if let Some(row) = cached {
            self.cached_shown.push(row);
        }
        self.briefs.push(brief.clone());
        self.live.push(Live {
            topic: topic.clone(),
            touched: now,
            cached_at,
        });
        (topic, brief, expired)
    }

    /// `meeting_brief`: validate and store a brief the model answered with, or one the
    /// actor shows from the cache (`cached`).
    #[allow(clippy::too_many_arguments)]
    pub fn add_brief(
        &mut self,
        title: &str,
        bullets: Vec<String>,
        topic: Option<TopicId>,
        parent: Option<BriefId>,
        source: BriefSource,
        cached: Option<BriefId>,
        at_ms: u64,
    ) -> Result<Brief, String> {
        check_bullets(&bullets)?;
        if title.trim().is_empty() {
            return Err("title is empty".into());
        }
        let brief = Brief {
            id: BriefId::new(),
            topic,
            title: title.trim().to_owned(),
            bullets: bullets.into_iter().map(|b| b.trim().to_owned()).collect(),
            source,
            at_ms,
            parent,
            cached,
        };
        self.briefs.push(brief.clone());
        Ok(brief)
    }

    /// The user tapped or the model answered `id`: off the live set (`Viewed`).
    pub fn view(&mut self, id: TopicId) -> Option<Topic> {
        self.take(id).map(|l| l.topic)
    }

    /// The user dismissed `id`: off the live set, its label remembered so the
    /// model cannot re-add it (`Dismissed`); a cached topic's row is barred
    /// from the alias scan for the rest of the meeting (05 §3).
    pub fn dismiss(&mut self, id: TopicId) -> Option<Topic> {
        let l = self.take(id)?;
        self.dismissed.push(l.topic.label.clone());
        if let Some(row) = l.topic.cached {
            self.cached_dismissed.push(row);
        }
        Some(l.topic)
    }

    fn take(&mut self, id: TopicId) -> Option<Live> {
        let i = self.live.iter().position(|l| l.topic.id == id)?;
        Some(self.live.remove(i))
    }

    /// An utterance was heard: every live label it contains (case-insensitive
    /// substring, 02 §0.5) is touched. Returns how many.
    pub fn touch(&mut self, text: &str, now: Instant) -> usize {
        let text = text.to_lowercase();
        let mut n = 0;
        for l in &mut self.live {
            if text.contains(&l.topic.label.to_lowercase()) {
                l.touched = now;
                n += 1;
            }
        }
        n
    }

    /// Topics untouched for `ttl`, removed (`Expired`), oldest first.
    pub fn expire(&mut self, now: Instant) -> Vec<Gone> {
        let ttl = self.ttl;
        let mut gone = Vec::new();
        self.live.retain(|l| {
            if now.saturating_duration_since(l.touched) >= ttl {
                gone.push(Gone {
                    topic: l.topic.id,
                    why: TopicGone::Expired,
                });
                false
            } else {
                true
            }
        });
        gone
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two() -> Vec<String> {
        vec!["one".into(), "two".into()]
    }

    /// Word and bullet counts, blank bullets, duplicates against live and
    /// dismissed (case-insensitive, trimmed).
    #[test]
    fn validation() {
        let t0 = Instant::now();
        let mut t = Topics::new(600, 6);
        assert!(t.insert("a b c d e", "x", two(), None, t0, 0).is_err());
        assert!(t.insert(" ", "x", two(), None, t0, 0).is_err());
        assert!(t.insert("SRE-2386", "x", vec!["only".into()], None, t0, 0).is_err());
        assert!(t.insert("SRE-2386", "x", vec!["a".into(), " ".into()], None, t0, 0).is_err());
        assert!(
            t.insert("SRE-2386", "x", vec!["1".into(), "2".into(), "3".into(), "4".into()], None, t0, 0)
                .is_err()
        );
        let (topic, brief, expired) = t.insert("  SRE-2386 ", "x", two(), None, t0, 5).unwrap();
        assert!(expired.is_empty());
        assert_eq!(topic.label, "SRE-2386");
        assert_eq!((topic.brief, brief.topic), (Some(brief.id), Some(topic.id)));
        assert_eq!(brief.source, BriefSource::Auto);
        assert_eq!(
            t.insert("sre-2386", "x", two(), None, t0, 6).unwrap_err(),
            "topic already live"
        );
        assert!(t.dismiss(topic.id).is_some());
        assert_eq!(
            t.insert("SRE-2386", "x", two(), None, t0, 7).unwrap_err(),
            "topic already dismissed"
        );
        assert_eq!(t.dismissed_labels(), ["SRE-2386"]);
        assert!(t.live().is_empty());
        assert_eq!(t.briefs().len(), 1, "the precomputed brief stays in the store");
        assert!(t.add_brief("t", vec!["a".into()], None, None, BriefSource::Typed, None, 0).is_err());
        assert!(t.add_brief(" ", two(), None, None, BriefSource::Typed, None, 0).is_err());
    }

    /// At `max`, the oldest *untouched* topic expires — a re-touched older
    /// topic survives a younger one; TTL expiry is by touch time; touch is a
    /// case-insensitive substring over the live labels only.
    #[test]
    fn expire_order_and_touch() {
        let t0 = Instant::now();
        let s = Duration::from_secs;
        let mut t = Topics::new(600, 2);
        let (a, _, _) = t.insert("Alpha", "x", two(), None, t0, 0).unwrap();
        let (b, _, _) = t.insert("DR cutover", "x", two(), None, t0 + s(1), 1).unwrap();
        assert_eq!(t.touch("the dr CUTOVER is Friday, alpha", t0 + s(2)), 2);
        assert_eq!(t.touch("nothing", t0 + s(3)), 0);
        // Both were touched at +2; Alpha again at +4, so the older-created Alpha
        // outlives the younger DR cutover when Gamma needs the slot.
        assert_eq!(t.touch("alpha again", t0 + s(4)), 1);
        let (c, _, expired) = t.insert("Gamma", "x", two(), None, t0 + s(5), 5).unwrap();
        assert_eq!(
            expired,
            [Gone {
                topic: b.id,
                why: TopicGone::Expired
            }]
        );
        assert_eq!(t.live_labels(), ["Alpha", "Gamma"]);
        assert!(t.topic(a.id).is_some() && t.topic(b.id).is_none());
        // TTL: Alpha (touched at +4) expires at +604, Gamma (created at +5) at +605.
        assert!(t.expire(t0 + s(603)).is_empty());
        assert_eq!(t.expire(t0 + s(604)).len(), 1);
        assert_eq!(t.live_labels(), ["Gamma"]);
        assert_eq!(t.expire(t0 + s(700)), [Gone { topic: c.id, why: TopicGone::Expired }]);
        assert!(t.live().is_empty());
        // Expired labels are not dismissed: the model may bring them back.
        assert!(t.insert("Gamma", "x", two(), None, t0 + s(701), 701).is_ok());
        assert!(t.view(t.live()[0].id).is_some());
        assert!(t.dismissed_labels().is_empty());
    }

    fn row(title: &str, updated_at: &str) -> CachedBrief {
        CachedBrief {
            id: BriefId::new(),
            title: title.into(),
            bullets: two(),
            aliases: vec![title.into()],
            tags: vec![],
            source: "user".into(),
            created_at: updated_at.into(),
            updated_at: updated_at.into(),
            uses: 0,
            last_used_at: None,
        }
    }

    /// A cached topic obeys `max` like an `Auto` one, is `source: Cache` with
    /// `cached` set, is never re-added once shown or dismissed this meeting
    /// (even under a different alias), and renders its age in the batch line;
    /// a `brief_put` over its row refreshes the stored brief and names the topic.
    #[test]
    fn insert_cached_rules() {
        let t0 = Instant::now();
        let mut t = Topics::new(600, 2);
        let now: DateTime<Utc> = "2026-09-20T12:00:00Z".parse().unwrap();
        let sre = row("SRE-2386", "2026-09-17T11:30:00Z");
        let keda = row("KEDA", "2026-09-20T09:00:00Z");
        let (topic, brief, _) = t.insert_cached("sre-2386", &sre, t0, 1).unwrap();
        assert_eq!((topic.label.as_str(), topic.cached, brief.cached), ("sre-2386", Some(sre.id), Some(sre.id)));
        assert_eq!((brief.source, brief.topic), (BriefSource::Cache, Some(topic.id)));
        assert!(t.cached_blocked(sre.id));
        assert!(t.insert_cached("two three eight six", &sre, t0, 2).is_err(), "already shown");
        let (_, _, _) = t.insert_cached("KEDA", &keda, t0 + Duration::from_secs(1), 3).unwrap();
        assert_eq!(t.batch_labels(now), ["sre-2386 (cached 3d)", "KEDA (cached 3h)"]);
        // `max` = 2: an Auto topic expires the oldest untouched cached one.
        let (_, _, expired) = t.insert("Gamma", "x", two(), None, t0 + Duration::from_secs(2), 4).unwrap();
        assert_eq!(expired[0].topic, topic.id);
        assert!(t.cached_blocked(sre.id), "expired stays shown for the meeting");
        // Dismiss the KEDA topic → its row is barred.
        let keda_topic = t.live().into_iter().find(|x| x.label == "KEDA").unwrap();
        assert!(t.dismiss(keda_topic.id).is_some());
        assert!(t.cached_blocked(keda.id));
        assert!(t.insert_cached("keda scaling", &keda, t0, 5).is_err());
        // A new `Topics` (next meeting) admits it again.
        assert!(Topics::new(600, 6).insert_cached("KEDA", &keda, t0, 0).is_ok());
        // refresh: a live topic's brief takes the new bullets; the topic id comes back.
        let dana = row("Dana", "2026-09-20T11:00:00Z");
        let (dt, _, _) = t.insert_cached("Dana", &dana, t0 + Duration::from_secs(3), 6).unwrap();
        let mut dana2 = dana.clone();
        dana2.bullets = vec!["new".into(), "bullets".into()];
        assert_eq!(t.refresh_cached(&dana2), Some(Some(dt.id)));
        assert_eq!(t.brief(dt.brief.unwrap()).unwrap().bullets, ["new", "bullets"]);
        assert_eq!(t.refresh_cached(&row("Other", "2026-09-20T11:00:00Z")), None);
        // An Auto topic written to the cache carries the row too and blocks the scan.
        let mut t = Topics::new(600, 6);
        let (at, ab, _) = t.insert("Auto one", "x", two(), Some(dana.id), t0, 0).unwrap();
        assert_eq!((at.cached, ab.cached, ab.source), (Some(dana.id), Some(dana.id), BriefSource::Auto));
        assert!(t.cached_blocked(dana.id));
    }

    /// The `briefs.jsonl` line shapes of §7.
    #[test]
    fn record_shape() {
        let r = Record::Removed {
            topic: TopicId(uuid::Uuid::nil()),
            why: TopicGone::Viewed,
            at_ms: 7,
        };
        assert_eq!(
            serde_json::to_string(&r).unwrap(),
            r#"{"removed":{"topic":"00000000-0000-0000-0000-000000000000","why":"Viewed","at_ms":7}}"#
        );
        assert!(serde_json::to_string(&Record::Topic(Topic {
            id: TopicId::new(),
            label: "x".into(),
            at_ms: 0,
            brief: None,
            cached: None,
        }))
        .unwrap()
        .starts_with(r#"{"topic":{"#));
    }
}
