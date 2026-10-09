// 144-E2 owns this file.
//! The audio overview (design 144 §10 W-l, contract §2): the two-host script the overview turn
//! `show`ed as `overview-script.md` is cut into speaker turns, each turn synthesised in its host's
//! voice by the `voice` actor (one `SynthesizeDialogue`: PCM joined with 350 ms silences, encoded
//! once to MP3 24 kHz mono) and `show`n as `overview.mp3` beside the script. Voices come from
//! `notebooks.audio_voices` ∩ what the checkpoint ships; with one voice both hosts read in it and
//! the card says so.

use kameo::actor::ActorRef;
use runtime::{Runtime, SessionId};
use serde::Deserialize;

/// `notebooks.audio_voices` (144 §14): the host voices in order; default the one voice the checkpoint ships.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// Voice names (`voice_status` lists the available ones).
    pub audio_voices: Vec<String>,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            audio_voices: vec!["peter_yearsley".to_owned()],
        }
    }
}

impl AudioConfig {
    /// The `notebooks` section, defaults when absent or unreadable.
    pub fn from_runtime(rt: &Runtime) -> Self {
        if rt.config.contains("notebooks") {
            rt.section::<Self>("notebooks").unwrap_or_default()
        } else {
            Self::default()
        }
    }
}

/// The script's speaker turns: `(speaker, text)` for every paragraph opening `**Name:**`; following
/// paragraphs without a speaker join the previous turn; text before the first speaker is dropped.
pub fn parse_script(script: &str) -> Vec<(String, String)> {
    let mut turns: Vec<(String, String)> = Vec::new();
    for para in script.split("\n\n") {
        let p = para.trim();
        if p.is_empty() || p.starts_with('#') {
            continue;
        }
        match speaker_line(p) {
            Some((who, text)) => turns.push((who.to_owned(), text.to_owned())),
            None => {
                if let Some(last) = turns.last_mut() {
                    last.1.push('\n');
                    last.1.push_str(p);
                }
            }
        }
    }
    turns.retain(|(_, t)| !t.trim().is_empty());
    turns
}

/// `**Ada:** text` / `**Ada**: text` / `Ada: text` (a one-word name) → `(Ada, text)`.
fn speaker_line(p: &str) -> Option<(&str, &str)> {
    if let Some(rest) = p.strip_prefix("**") {
        let (name, after) = rest.split_once("**")?;
        let name = name.trim().trim_end_matches(':').trim();
        let text = after.trim_start().strip_prefix(':').unwrap_or(after).trim();
        return (!name.is_empty() && !name.contains(' ')).then_some((name, text));
    }
    let (name, text) = p.split_once(':')?;
    let name = name.trim();
    (!name.is_empty()
        && name.len() <= 24
        && !name.contains(' ')
        && name.chars().next().is_some_and(char::is_uppercase))
    .then_some((name, text.trim()))
}

/// Voice per speaker (speakers in order of first appearance) and the card note: two or more usable
/// voices alternate by speaker; fewer → the one voice (the first usable, else `fallback`) reads
/// every host and the note says so.
pub fn assign_voices(
    speakers: &[String],
    configured: &[String],
    available: &[String],
    fallback: &str,
) -> (Vec<String>, Option<String>) {
    let usable: Vec<&String> = configured
        .iter()
        .filter(|v| available.contains(v))
        .collect();
    if usable.len() >= 2 {
        return (
            speakers
                .iter()
                .enumerate()
                .map(|(i, _)| usable[i % usable.len()].clone())
                .collect(),
            None,
        );
    }
    let one = usable.first().map_or(fallback, |v| v.as_str()).to_owned();
    let note = format!("one voice available — both hosts read by {one}");
    (vec![one; speakers.len()], Some(note))
}

/// Script → `overview.mp3` shown in `session`: `(call_id, version, note)`.
pub async fn render(
    rt: &Runtime,
    session: SessionId,
    script: &str,
) -> anyhow::Result<(String, u32, Option<String>)> {
    let turns = parse_script(script);
    anyhow::ensure!(
        !turns.is_empty(),
        "the script has no `**Host:**` speaker turns"
    );
    let voice = ActorRef::<voice::Voice>::lookup(voice::VOICE_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(|| anyhow::anyhow!("voice is not running (voice.enabled)"))?;
    let mut status = voice
        .ask(voice::Status)
        .await
        .map_err(|e| anyhow::anyhow!("voice: {e}"))?;
    if status.voices.is_empty() {
        // The checkpoint lists its voices once loaded; a one-word clip loads it.
        let _ = voice
            .ask(voice::Synthesize {
                text: "Loading.".to_owned(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("voice: {e}"))?;
        status = voice
            .ask(voice::Status)
            .await
            .map_err(|e| anyhow::anyhow!("voice: {e}"))?;
    }
    let cfg = AudioConfig::from_runtime(rt);
    let mut speakers: Vec<String> = Vec::new();
    for (who, _) in &turns {
        if !speakers.contains(who) {
            speakers.push(who.clone());
        }
    }
    let (voices, note) = assign_voices(&speakers, &cfg.audio_voices, &status.voices, &status.voice);
    let dialogue: Vec<voice::DialogueTurn> = turns
        .iter()
        .map(|(who, text)| voice::DialogueTurn {
            voice: speakers
                .iter()
                .position(|s| s == who)
                .map(|i| voices[i].clone()),
            text: text.clone(),
        })
        .collect();
    let clip = voice
        .ask(voice::SynthesizeDialogue { turns: dialogue })
        .await
        .map_err(|e| anyhow::anyhow!("tts: {e}"))?
        .ok_or_else(|| anyhow::anyhow!("the script has nothing to say"))?;
    let dir = rt.data_dir.join("notebooks").join("tmp");
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join(format!("overview-{session}.mp3"));
    tokio::fs::write(&path, &clip.bytes).await?;
    let shown = crate::overviews::show_path(
        session,
        &path,
        "overview.mp3",
        &format!("Audio overview · {} min", clip.duration_ms.div_ceil(60_000)),
    )
    .await;
    let _ = tokio::fs::remove_file(&path).await;
    let (call_id, version, _) = shown?;
    Ok((call_id, version, note))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_splits_into_speaker_turns() {
        let script = "# Audio overview\n\nIntro line without a speaker.\n\n**Ada:** Welcome to the DR notebook \
                      [runbook](theuth://sources/S1#b1).\n\n**Ben:** Thanks Ada.\nStill Ben.\n\nA continuation paragraph.\n\n\
                      **Ada**: Back to me.\n\nBen: Plain colon works too.";
        let turns = parse_script(script);
        let names: Vec<&str> = turns.iter().map(|(w, _)| w.as_str()).collect();
        assert_eq!(names, ["Ada", "Ben", "Ada", "Ben"]);
        assert_eq!(
            turns[0].1,
            "Welcome to the DR notebook [runbook](theuth://sources/S1#b1)."
        );
        assert_eq!(
            turns[1].1,
            "Thanks Ada.\nStill Ben.\nA continuation paragraph."
        );
        assert_eq!(turns[2].1, "Back to me.");
        assert_eq!(turns[3].1, "Plain colon works too.");
        assert!(
            parse_script("No speakers here.\n\nJust prose: with a colon mid sentence").is_empty()
        );
    }

    #[test]
    fn voices_alternate_or_fall_back_to_one_with_a_note() {
        let hosts = vec!["Ada".to_owned(), "Ben".to_owned()];
        let (v, note) = assign_voices(
            &hosts,
            &["peter_yearsley".to_owned(), "alba".to_owned()],
            &["alba".to_owned(), "peter_yearsley".to_owned()],
            "peter_yearsley",
        );
        assert_eq!(v, ["peter_yearsley", "alba"]);
        assert!(note.is_none());
        let (v, note) = assign_voices(
            &hosts,
            &["peter_yearsley".to_owned(), "alba".to_owned()],
            &["peter_yearsley".to_owned()],
            "peter_yearsley",
        );
        assert_eq!(v, ["peter_yearsley", "peter_yearsley"]);
        assert_eq!(
            note.as_deref(),
            Some("one voice available — both hosts read by peter_yearsley")
        );
        // Nothing configured is installed: the actor's configured voice reads both.
        let (v, _) = assign_voices(
            &hosts,
            &["ghost".to_owned()],
            &["marius".to_owned()],
            "marius",
        );
        assert_eq!(v, ["marius", "marius"]);
    }
}
