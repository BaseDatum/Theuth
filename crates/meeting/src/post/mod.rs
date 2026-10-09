//! The after-meeting pipeline (04 §2): re-transcribe both tracks with word times,
//! diarize the system track, align, write `transcript.json`, index, `Review`.
//! One tokio task per finished meeting, off the actor's mailbox; it owns the
//! files and the manifest state for its stages and publishes its own events, so
//! the actor only keeps the `JoinHandle` (to refuse a second run and to resume
//! `Post(_)` manifests at start — the pipeline is idempotent from the Opus files).

pub mod align;
pub mod diarize;
pub mod transcribe;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use runtime::Runtime;
use tokio::sync::mpsc::UnboundedSender;
use voice::Engine;

use crate::speakers::Profiles;
use crate::store::{Manifest, Stem, Store};
use crate::transcript::{self, ME, Proposed, SpeakerLabel, Transcript, Word, default_name};
use crate::{MeetingEvent, MeetingId, MeetingState, PostStage, Utterance};

/// Where the pipeline finds its models and profiles.
#[derive(Clone, Debug)]
pub struct Paths {
    /// Parakeet TDT checkpoint (`voice::TDT_MODEL_SUBDIR`).
    pub tdt: PathBuf,
    /// The models root (`meeting.models_dir`, default `<data_dir>/models`): the diariser
    /// under `nemotron-diarization/`, wespeaker under `pyannote/` (`diarize::models`).
    pub models: PathBuf,
    /// `<data_dir>/meetings/speakers/`.
    pub speakers: PathBuf,
    /// `<data_dir>`, for `models::onnx::ort_lib`.
    pub data_dir: PathBuf,
}

/// A running pipeline.
pub struct Job {
    /// Its meeting.
    pub meeting: MeetingId,
    /// The task.
    pub handle: tokio::task::JoinHandle<()>,
}

/// Most mic audio folded into the `me` embedding (its word regions, in order).
const ME_EMBED_SAMPLES: usize = 30 * transcribe::RATE as usize;

struct Ctx {
    rt: Runtime,
    store: Arc<Store>,
    paths: Paths,
    progress: UnboundedSender<(PostStage, u8)>,
}

/// Spawn the pipeline for `manifest` (state is set to `Post(Transcribing)` here).
pub fn spawn(rt: Runtime, store: Arc<Store>, manifest: Manifest, paths: Paths) -> Job {
    let meeting = manifest.id;
    let handle = tokio::spawn(async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let forward = {
            let rt = rt.clone();
            tokio::spawn(async move {
                let mut last = None;
                while let Some((stage, pct)) = rx.recv().await {
                    if last == Some((stage, pct)) {
                        continue;
                    }
                    last = Some((stage, pct));
                    rt.publish(MeetingEvent::PostProgress {
                        meeting,
                        stage,
                        pct,
                    })
                    .await;
                }
            })
        };
        let cx = Ctx {
            rt,
            store,
            paths,
            progress: tx,
        };
        let mut manifest = manifest;
        manifest.failure = None;
        if let Err((stage, e)) = run(&cx, &mut manifest).await {
            tracing::warn!(meeting = %meeting, "post pipeline: {stage:?}: {e:#}");
            manifest.state = MeetingState::Failed;
            manifest.failure = Some(format!("{stage:?}: {e:#}"));
            save(&cx, &manifest).await;
        }
        drop(cx);
        let _ = forward.await;
    });
    Job { meeting, handle }
}

async fn save(cx: &Ctx, m: &Manifest) {
    if let Err(e) = cx.store.save(m).await {
        tracing::warn!(meeting = %m.id, "manifest: {e:#}");
    }
    cx.rt
        .publish(MeetingEvent::State {
            meeting: m.id,
            state: m.state,
            degraded: m.degraded.clone(),
        })
        .await;
}

async fn stage(cx: &Ctx, m: &mut Manifest, stage: PostStage, pct: u8) {
    m.state = MeetingState::Post(stage);
    save(cx, m).await;
    let _ = cx.progress.send((stage, pct));
}

/// Both tracks' words and their 16 kHz PCM (mic, system).
struct Transcribed {
    mic: Vec<Word>,
    system: Vec<Word>,
    mic_pcm: Vec<f32>,
    system_pcm: Vec<f32>,
}

async fn run(cx: &Ctx, m: &mut Manifest) -> Result<(), (PostStage, anyhow::Error)> {
    let dir = cx.store.meeting_dir(m.id);
    let file = |stem: Stem| {
        dir.join(
            m.tracks
                .iter()
                .find(|t| t.track == stem)
                .map_or(stem.file(), |t| t.file.as_str()),
        )
    };
    let (mic_path, system_path) = (file(Stem::Mic), file(Stem::System));

    // 1. Transcribing (0–60 %). The Engine loads first: it runs the process-wide ort init.
    stage(cx, m, PostStage::Transcribing, 0).await;
    let progress = cx.progress.clone();
    let (tdt, data_dir) = (cx.paths.tdt.clone(), cx.paths.data_dir.clone());
    let mut t = tokio::task::spawn_blocking(move || -> anyhow::Result<Transcribed> {
        let engine = Engine::load(&tdt, &data_dir)?;
        let mic_pcm = transcribe::pcm16k(&mic_path)?;
        let system_pcm = transcribe::pcm16k(&system_path)?;
        let total = (mic_pcm.len() + system_pcm.len()).max(1);
        let report = |done_samples: usize| {
            let _ = progress.send((
                PostStage::Transcribing,
                (done_samples * 60 / total).min(60) as u8,
            ));
        };
        let mic = transcribe::words(&engine, &mic_pcm, |d, n| {
            report(mic_pcm.len() * d / n.max(1));
        })?;
        let system = transcribe::words(&engine, &system_pcm, |d, n| {
            report(mic_pcm.len() + system_pcm.len() * d / n.max(1));
        })?;
        Ok(Transcribed {
            mic,
            system,
            mic_pcm,
            system_pcm,
        })
    })
    .await
    .map_err(|e| (PostStage::Transcribing, anyhow::anyhow!("{e}")))
    .and_then(|r| r.map_err(|e| (PostStage::Transcribing, e)))?;

    // 2. Diarizing (60–90 %), skipped without the models (04 §0.1).
    let models = diarize::models(&cx.paths.models, &cx.paths.data_dir);
    let note = models.is_none().then(|| {
        format!(
            "Diarization skipped: models not found under {} ({}/{} and {}/{}). \
             Every system segment is Speaker 1; relabel by hand or place the models and re-run.",
            cx.paths.models.display(),
            diarize::DIAR_DIR,
            diarize::DIAR_MODEL,
            diarize::EMBED_DIR,
            diarize::EMBEDDING
        )
    });
    let (mut diarized, me_emb) = match models {
        None => (diarize::Diarized::default(), None),
        Some(models) => {
            stage(cx, m, PostStage::Diarizing, 60).await;
            let progress = cx.progress.clone();
            let system_pcm = std::mem::take(&mut t.system_pcm);
            let mic_pcm = std::mem::take(&mut t.mic_pcm);
            let mic_words = t.mic.clone();
            tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
                let d = diarize::run(&system_pcm, &models, |p| {
                    let _ =
                        progress.send((PostStage::Diarizing, 60 + (u32::from(p) * 30 / 100) as u8));
                })?;
                let me = me_embedding(&models, &mic_pcm, &mic_words)?;
                Ok((d, me))
            })
            .await
            .map_err(|e| (PostStage::Diarizing, anyhow::anyhow!("{e}")))
            .and_then(|r| r.map_err(|e| (PostStage::Diarizing, e)))?
        }
    };

    // 3. Aligning (90–100 %). 07 §5: the offline labels are renamed to agree with the live
    // ones first, so "Speaker 2" is one voice in the session and in the record.
    stage(cx, m, PostStage::Aligning, 90).await;
    let live: Vec<Utterance> =
        crate::read_jsonl(&dir.join(crate::LIVE_LOG), usize::MAX).unwrap_or_default();
    let renamed = align::reconcile(&live, &diarized.spans);
    for s in &mut diarized.spans {
        if let Some(id) = renamed.get(&s.speaker) {
            s.speaker.clone_from(id);
        }
    }
    let segments = align::segments(&t.mic, &t.system, &diarized.spans);
    let mut embeddings: HashMap<String, Vec<f32>> = diarized
        .embeddings
        .into_iter()
        .map(|(id, e)| (renamed.get(&id).cloned().unwrap_or(id), e))
        .collect();
    if let Some(e) = me_emb {
        embeddings.insert(ME.to_owned(), e);
    }
    let profiles = Profiles::load(&cx.paths.speakers).map_err(|e| (PostStage::Aligning, e))?;
    let mut ids: Vec<String> = segments
        .iter()
        .filter(|s| s.speaker != ME)
        .map(|s| s.speaker.clone())
        .collect();
    ids.sort_by_key(|id| id[1..].parse::<u32>().unwrap_or(u32::MAX));
    ids.dedup();
    let mut speakers = vec![SpeakerLabel {
        id: ME.into(),
        name: default_name(ME),
        profile: None,
        proposed: None,
    }];
    speakers.extend(ids.into_iter().map(|id| {
        SpeakerLabel {
            name: default_name(&id),
            proposed: embeddings
                .get(&id)
                .and_then(|e| profiles.best_match(e))
                .map(|(profile, name, score)| Proposed {
                    profile,
                    name,
                    score,
                }),
            id,
            profile: None,
        }
    }));
    let transcript = Transcript {
        meeting: m.id,
        speakers,
        segments,
        reviewed_at: None,
        summarized_at: None,
        diarized: note.is_none(),
        note,
    };
    std::fs::write(
        dir.join(transcript::EMBEDDINGS),
        serde_json::to_vec(&embeddings).map_err(|e| (PostStage::Aligning, e.into()))?,
    )
    .map_err(|e| (PostStage::Aligning, e.into()))?;
    cx.store
        .save_transcript(&transcript)
        .await
        .map_err(|e| (PostStage::Aligning, e))?;
    let _ = cx.progress.send((PostStage::Aligning, 100));
    m.state = MeetingState::Review;
    save(cx, m).await;
    cx.rt
        .publish(MeetingEvent::ReviewReady { meeting: m.id })
        .await;
    Ok(())
}

/// The mic track's voice: its word regions concatenated (≤ 30 s) through the
/// embedding model. `None` when the user said nothing.
fn me_embedding(
    models: &diarize::Models,
    pcm: &[f32],
    words: &[Word],
) -> anyhow::Result<Option<Vec<f32>>> {
    let rate = transcribe::RATE as usize;
    let mut speech: Vec<f32> = Vec::new();
    for w in words {
        let a = (w.t0_ms as usize * rate / 1000).min(pcm.len());
        let b = (w.t1_ms as usize * rate / 1000).min(pcm.len());
        speech.extend_from_slice(&pcm[a..b]);
        if speech.len() >= ME_EMBED_SAMPLES {
            break;
        }
    }
    if speech.len() < rate {
        return Ok(None);
    }
    let mut e = diarize::Embedder::open(models)?;
    e.compute(&diarize::to_i16(&speech)).map(Some)
}

/// 156 D14: the voice in `spans` (ms on the track at `path`) through the embedding model,
/// as [`me_embedding`] does for the mic words: decoded only as far as the last span
/// ([`transcribe::opus_ranges`]), concatenated, resampled to 16 kHz. Blocking (seconds).
pub fn embed_spans(
    path: &Path,
    spans: &[(u64, u64)],
    models: &diarize::Models,
) -> anyhow::Result<Vec<f32>> {
    let cut = transcribe::opus_ranges(path, spans)?;
    let speech = voice::resample(cut, transcribe::OPUS_RATE, transcribe::RATE)?;
    let mut e = diarize::Embedder::open(models)?;
    e.compute(&diarize::to_i16(&speech))
}
