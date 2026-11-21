use std::path::PathBuf;
use crate as talk;
use crate::rpc_support::{dictation, history_store, talk, transcriber, voice};
use agent::{Agents, Polish, PolishReply, Rewrite};
use agent::rpc_support::RpcServerExt;
use base64::Engine as _;
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use rpc_core::{Server, ask, failed, peer, send};
use runtime::SessionId;
use ::voice::{SpeechId, VoiceStatus};

#[rpc_core_macros::rpc_descriptors(owner = "talk", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait SpeechApi {
    /// Read `text` (markdown) aloud in `session`'s name, stopping whatever is
    /// speaking first (43 §5). Replies with the utterance's id at once; audio
    /// follows as `SpeechAudio` events on `subscribe_events`.
    #[method(name = "speak", with_extensions)]
    async fn speak(&self, session: SessionId, text: String) -> RpcResult<SpeechId>;
    /// Stop utterance `id`, or whatever is speaking when `null`. Idempotent.
    #[method(name = "stop_speaking")]
    async fn stop_speaking(&self, id: Option<SpeechId>) -> RpcResult<()>;
    /// Model state, configured voice, available voices and the utterance in
    /// progress (43 §5).
    #[method(name = "voice_status")]
    async fn voice_status(&self) -> RpcResult<VoiceStatus>;
    /// Transcribe an audio file stored by `put_attachment` (45 §7.1); `path`
    /// must be under `<data_dir>/attachments`. `NOT_RUNNING` when the voice
    /// plugin is off; a missing STT checkpoint is an error string.
    #[method(name = "transcribe")]
    async fn transcribe(&self, path: PathBuf) -> RpcResult<voice::Transcript>;
    /// Load the streaming dictation model (and the VAD) so Ctrl+D is instant;
    /// never opens the microphone; idempotent — replies with the current
    /// `ModelState`, `Loading` while the load is in flight (54 §0.5, §8).
    /// Frontends call it once after connect.
    #[method(name = "dictation_prewarm", with_extensions)]
    async fn dictation_prewarm(&self) -> RpcResult<voice::ModelState>;
    /// Start streaming text (54 §8); text arrives as `DictationText` events on
    /// `subscribe_events`. `sample_rate: None` opens the daemon's microphone —
    /// local peers only; a relay peer gets "microphone is on the harness host;
    /// pass sample_rate to stream audio". `Some(rate)` (a multiple of 50)
    /// means the caller streams its own mono i16 PCM at `rate` Hz with
    /// `dictation_audio`; the dictation stops when that connection closes.
    /// Other error strings: `already dictating`, `voice is disabled`, a
    /// missing/failed model (with the expected path), a mic open failure.
    /// `opts` (153 §14.1): surface, destination, app profile, slash commands, press offset;
    /// `None` = today's Composer dictation.
    #[method(name = "dictation_start", with_extensions)]
    async fn dictation_start(
        &self,
        sample_rate: Option<u32>,
        opts: Option<voice::DictationOpts>,
    ) -> RpcResult<voice::DictationId>;
    /// Feed remote dictation `id`: `pcm` is base64 of little-endian mono i16
    /// samples at the rate given to `dictation_start`, ~100 ms per call.
    /// `Err` when `id` is not a running remote dictation or `pcm` is not
    /// whole i16s.
    #[method(name = "dictation_audio", with_extensions)]
    async fn dictation_audio(&self, id: voice::DictationId, pcm: String) -> RpcResult<()>;
    /// Stop dictation `id`, flushing the open utterance first; `Finished`
    /// follows as an event. Idempotent: an unknown or finished id is `Ok`
    /// (54 §8). Any authenticated peer may stop, including a relay peer.
    #[method(name = "dictation_stop", with_extensions)]
    async fn dictation_stop(&self, id: voice::DictationId) -> RpcResult<()>;
    /// The dictation model's state and the running dictation, if any (54 §8):
    /// how a second frontend learns it is "dictating elsewhere". Open to
    /// relay peers.
    #[method(name = "dictation_status", with_extensions)]
    async fn dictation_status(&self) -> RpcResult<voice::DictationState>;
    /// 153 §6.1: the surface sent `sent_text` after dictation `id`; the engine diffs it
    /// against what it delivered and learns single-word corrections. Any authenticated peer.
    #[method(name = "dictation_correction", with_extensions)]
    async fn dictation_correction(
        &self,
        id: voice::DictationId,
        sent_text: String,
    ) -> RpcResult<()>;
    /// 153 §6.5: the newest `limit` dictation rows (raw/shaped/polished). Local peers only.
    #[method(name = "dictation_history", with_extensions)]
    async fn dictation_history(&self, limit: Option<u32>) -> RpcResult<Vec<voice::HistoryRow>>;
    /// 153 §6.7: forget one row (and its audio), or everything when `None`. Local peers only.
    #[method(name = "dictation_history_forget", with_extensions)]
    async fn dictation_history_forget(&self, id: Option<voice::DictationId>) -> RpcResult<()>;
    /// 153 §6.4: the dictation's audio (one utterance, or all) as base64 i16 16 kHz;
    /// `Err("no audio")` when evicted. Local peers only.
    #[method(name = "dictation_audio_get", with_extensions)]
    async fn dictation_audio_get(
        &self,
        id: voice::DictationId,
        utterance: Option<u32>,
    ) -> RpcResult<String>;
    /// 153 §5.6: user + learned dictionary entries, optionally one scope.
    #[method(name = "dictionary_list")]
    async fn dictionary_list(&self, scope: Option<voice::Scope>) -> RpcResult<Vec<voice::Entry>>;
    /// 153 §5.6: upsert by canonical + scope; replies with the stored entry.
    #[method(name = "dictionary_put")]
    async fn dictionary_put(&self, entry: voice::Entry) -> RpcResult<voice::Entry>;
    /// 153 §5.6: delete one entry; `false` when unknown.
    #[method(name = "dictionary_delete")]
    async fn dictionary_delete(&self, id: voice::EntryId) -> RpcResult<bool>;
    /// 153 §6.3 step 4: answer a `DictionarySuggestion` event.
    #[method(name = "dictionary_suggestion_answer")]
    async fn dictionary_suggestion_answer(
        &self,
        id: voice::SuggestionId,
        answer: voice::SuggestionAnswer,
    ) -> RpcResult<()>;
    /// 153 §8.1: every input device the host enumerates, the default flagged.
    #[method(name = "list_input_devices")]
    async fn list_input_devices(&self) -> RpcResult<Vec<voice::InputDevice>>;
    /// 153 §8.1: persist `voice.input_devices` (ordered; `[]` = system default only).
    #[method(name = "set_input_devices")]
    async fn set_input_devices(&self, names: Vec<String>) -> RpcResult<()>;
    /// 153 §4.5: persist `voice.shape.<surface>`.
    #[method(name = "set_shape")]
    async fn set_shape(&self, surface: voice::Surface, preset: voice::Preset) -> RpcResult<()>;
    /// 203: persist `voice.shape.enabled`; `false` bypasses the shaper on every surface.
    #[method(name = "set_shape_enabled")]
    async fn set_shape_enabled(&self, enabled: bool) -> RpcResult<()>;
    /// 202 §5: persist `voice.stt_engine` and apply it at once; utterances already running keep
    /// the engine they started with.
    #[method(name = "set_stt_engine")]
    async fn set_stt_engine(&self, engine: voice::mlx::SttEngine) -> RpcResult<()>;
    /// 153 §7.2: persist `voice.end_commands`.
    #[method(name = "set_end_phrases")]
    async fn set_end_phrases(&self, phrases: voice::EndPhrases) -> RpcResult<()>;
    /// 153 §9: polish `text` for a human reader at `destination`; `AgentPrompt` is refused.
    /// Fails closed: `text: None` + `refused` means deliver the shaped text.
    #[method(name = "polish")]
    async fn polish(
        &self,
        text: String,
        destination: voice::Destination,
        session: Option<SessionId>,
    ) -> RpcResult<PolishReply>;
    /// 153 §13.6: rewrite `selection` per the spoken `instruction` in `app`; `thread` carries
    /// earlier (instruction, result) pairs of the same selection.
    #[method(name = "rewrite")]
    async fn rewrite(
        &self,
        instruction: String,
        selection: String,
        app: voice::AppProfile,
        thread: Option<Vec<(String, String)>>,
    ) -> RpcResult<PolishReply>;
    /// Start a live spoken conversation with `session` (76 §5.1); events arrive
    /// as `TalkEvent` on `subscribe_events`. Local peers pass `sink: "device"`
    /// and `sample_rate: null` (the daemon's speaker and mic); a relay peer must
    /// pass `sink: "client"` and the rate of the mono i16 PCM it will push with
    /// `talk_audio` — refused otherwise. One talk per daemon: a second start
    /// answers `already talking`; `talk is disabled` when the plugin is off.
    #[method(name = "talk_start", with_extensions)]
    async fn talk_start(
        &self,
        session: SessionId,
        sink: talk::Sink,
        sample_rate: Option<u32>,
    ) -> RpcResult<talk::TalkId>;
    /// Stop talk `id`; `TalkFinished` follows as an event. Idempotent. Any
    /// authenticated peer may stop, including a relay peer.
    #[method(name = "talk_stop", with_extensions)]
    async fn talk_stop(&self, id: talk::TalkId) -> RpcResult<()>;
    /// Feed a `sink: "client"` talk (76 §5.3): `pcm` is base64 of little-endian
    /// mono i16 samples at the rate given to `talk_start`, ~100 ms per call;
    /// `played_seq` is the highest `SpeechAudio.seq` whose playback ended on
    /// the client since the last call — the heard-prefix position.
    #[method(name = "talk_audio", with_extensions)]
    async fn talk_audio(
        &self,
        id: talk::TalkId,
        pcm: String,
        played_seq: Option<u32>,
    ) -> RpcResult<()>;
    /// The running talk, if any, and the streaming model's state (76 §5.1).
    /// Open to relay peers.
    #[method(name = "talk_status", with_extensions)]
    async fn talk_status(&self) -> RpcResult<talk::TalkState>;
}

#[async_trait]
impl SpeechApiServer for Server {

    async fn speak(
        &self,
        ext: &Extensions,
        session: SessionId,
        text: String,
    ) -> RpcResult<SpeechId> {
        self.authorize(ext, session).await?;
        // 43's button: a replacing utterance the client plays (76 §0.3 keeps this exactly).
        send(
            &voice()?,
            voice::Speak {
                session,
                text,
                queue: false,
                sink: voice::Sink::Client,
            },
        )
        .await
    }

    async fn stop_speaking(&self, id: Option<SpeechId>) -> RpcResult<()> {
        send(&voice()?, voice::Stop { id }).await
    }

    async fn voice_status(&self) -> RpcResult<VoiceStatus> {
        let mut status = send(&voice()?, voice::Status).await?;
        // The live list (`set_input_devices`), not the one read at startup.
        if let Ok(d) = dictation() {
            status.input_devices = send(&d, voice::InputDevicePrefs).await?;
        }
        Ok(status)
    }

    async fn transcribe(&self, path: PathBuf) -> RpcResult<voice::Transcript> {
        let real =
            std::fs::canonicalize(&path).map_err(|e| failed(format!("{}: {e}", path.display())))?;
        let inbox = std::fs::canonicalize(self.rt.data_dir.join("attachments"));
        if !inbox.is_ok_and(|dir| real.starts_with(dir)) {
            return Err(failed(format!("{}: not an attachment", path.display())));
        }
        send(&transcriber()?, voice::Transcribe { path: real }).await
    }

    async fn dictation_prewarm(&self, ext: &Extensions) -> RpcResult<voice::ModelState> {
        peer(ext)?;
        send(&dictation()?, voice::Prewarm).await
    }

    async fn dictation_start(
        &self,
        ext: &Extensions,
        sample_rate: Option<u32>,
        opts: Option<voice::DictationOpts>,
    ) -> RpcResult<voice::DictationId> {
        let peer = peer(ext)?;
        let source = match sample_rate {
            None if peer.remote() => {
                return Err(failed(
                    "microphone is on the harness host; pass sample_rate to stream audio",
                ));
            }
            None => voice::AudioSource::Mic,
            Some(sample_rate) => voice::AudioSource::Remote { sample_rate },
        };
        let actor = dictation()?;
        let id = send(
            &actor,
            voice::StartDictation {
                source,
                opts: opts.unwrap_or_default(),
            },
        )
        .await?;
        if source != voice::AudioSource::Mic {
            // A dropped connection must not hold the one-dictation slot.
            tokio::spawn(async move {
                peer.closed().await;
                let _ = send(&actor, voice::StopDictation { id }).await;
            });
        }
        Ok(id)
    }

    async fn dictation_audio(
        &self,
        ext: &Extensions,
        id: voice::DictationId,
        pcm: String,
    ) -> RpcResult<()> {
        peer(ext)?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(pcm)
            .map_err(|e| failed(format!("bad base64: {e}")))?;
        if bytes.len() % 2 != 0 {
            return Err(failed("pcm must be little-endian i16"));
        }
        let pcm = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes(*b))
            .collect();
        send(&dictation()?, voice::PushAudio { id, pcm }).await
    }

    async fn dictation_stop(&self, ext: &Extensions, id: voice::DictationId) -> RpcResult<()> {
        peer(ext)?;
        send(&dictation()?, voice::StopDictation { id }).await
    }

    async fn dictation_status(&self, ext: &Extensions) -> RpcResult<voice::DictationState> {
        peer(ext)?;
        send(&dictation()?, voice::DictationStatus).await
    }

    // ---- 153 dictation history / dictionary (A4 fills the bodies) ----------------------------

    async fn dictation_correction(
        &self,
        ext: &Extensions,
        id: voice::DictationId,
        sent_text: String,
    ) -> RpcResult<()> {
        peer(ext)?;
        send(&dictation()?, voice::DictationCorrection { id, sent_text }).await
    }

    async fn dictation_history(
        &self,
        ext: &Extensions,
        limit: Option<u32>,
    ) -> RpcResult<Vec<voice::HistoryRow>> {
        let store = history_store(ext).await?;
        store
            .list(limit.unwrap_or(50))
            .await
            .map_err(|e| failed(format!("{e:#}")))
    }

    async fn dictation_history_forget(
        &self,
        ext: &Extensions,
        id: Option<voice::DictationId>,
    ) -> RpcResult<()> {
        let store = history_store(ext).await?;
        store.forget(id).await.map_err(|e| failed(format!("{e:#}")))
    }

    async fn dictation_audio_get(
        &self,
        ext: &Extensions,
        id: voice::DictationId,
        utterance: Option<u32>,
    ) -> RpcResult<String> {
        let store = history_store(ext).await?;
        let pcm = store
            .audio(id, utterance)
            .await
            .map_err(|e| failed(format!("{e:#}")))?;
        let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
        Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    async fn dictionary_list(&self, scope: Option<voice::Scope>) -> RpcResult<Vec<voice::Entry>> {
        match send(&dictation()?, voice::DictionaryOp::List(scope)).await? {
            voice::DictionaryReply::Entries(e) => Ok(e),
            voice::DictionaryReply::Loading => Err(failed("dictionary loading")),
            other => Err(failed(format!("unexpected dictionary reply {other:?}"))),
        }
    }

    async fn dictionary_put(&self, entry: voice::Entry) -> RpcResult<voice::Entry> {
        match send(&dictation()?, voice::DictionaryOp::Put(entry)).await? {
            voice::DictionaryReply::Entry(e) => Ok(e),
            voice::DictionaryReply::Loading => Err(failed("dictionary loading")),
            other => Err(failed(format!("unexpected dictionary reply {other:?}"))),
        }
    }

    async fn dictionary_delete(&self, id: voice::EntryId) -> RpcResult<bool> {
        match send(&dictation()?, voice::DictionaryOp::Delete(id)).await? {
            voice::DictionaryReply::Deleted(d) => Ok(d),
            voice::DictionaryReply::Loading => Err(failed("dictionary loading")),
            other => Err(failed(format!("unexpected dictionary reply {other:?}"))),
        }
    }

    async fn dictionary_suggestion_answer(
        &self,
        id: voice::SuggestionId,
        answer: voice::SuggestionAnswer,
    ) -> RpcResult<()> {
        send(&dictation()?, voice::AnswerSuggestion { id, answer }).await
    }

    async fn list_input_devices(&self) -> RpcResult<Vec<voice::InputDevice>> {
        // Host enumeration is a blocking call into the audio backend.
        tokio::task::spawn_blocking(voice::mic::list_input_devices)
            .await
            .map_err(|e| failed(format!("device list: {e}")))
    }

    async fn set_input_devices(&self, names: Vec<String>) -> RpcResult<()> {
        let value = serde_json::to_string(&names).map_err(|e| failed(e.to_string()))?;
        self.write_voice_config(&["input_devices"], &value)?;
        send(&dictation()?, voice::Setting::InputDevices(names)).await
    }

    async fn set_shape(&self, surface: voice::Surface, preset: voice::Preset) -> RpcResult<()> {
        let key = serde_json::to_value(surface)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .ok_or_else(|| failed("bad surface"))?;
        let value = serde_json::to_value(preset)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .ok_or_else(|| failed("bad preset"))?;
        self.write_voice_config(&["shape", &key], &value)?;
        send(&dictation()?, voice::Setting::Shape(surface, preset)).await
    }

    async fn set_shape_enabled(&self, enabled: bool) -> RpcResult<()> {
        self.write_voice_config(&["shape", "enabled"], if enabled { "true" } else { "false" })?;
        send(&dictation()?, voice::Setting::ShapeEnabled(enabled)).await
    }

    async fn set_stt_engine(&self, engine: voice::mlx::SttEngine) -> RpcResult<()> {
        let value = match engine {
            voice::mlx::SttEngine::Onnx => "onnx",
            voice::mlx::SttEngine::Mlx => "mlx",
        };
        self.write_voice_config(&["stt_engine"], value)?;
        let data_dir = self.rt.data_dir.clone();
        // `select` stats the weights and may start the engine thread: off the async workers.
        tokio::task::spawn_blocking(move || voice::mlx::switch(engine, &data_dir))
            .await
            .map_err(|e| failed(e.to_string()))
    }

    async fn set_end_phrases(&self, phrases: voice::EndPhrases) -> RpcResult<()> {
        let json = |v: serde_json::Value| v.to_string();
        self.write_voice_config(
            &["end_commands", "send"],
            &json(phrases.send.clone().into()),
        )?;
        self.write_voice_config(
            &["end_commands", "scratch"],
            &json(phrases.scratch.clone().into()),
        )?;
        self.write_voice_config(
            &["end_commands", "literal"],
            &json(phrases.literal.clone().into()),
        )?;
        send(&dictation()?, voice::Setting::EndPhrases(phrases)).await
    }

    // ---- 153 polish / rewrite (A5 fills the bodies) ------------------------------------------

    async fn polish(
        &self,
        text: String,
        destination: voice::Destination,
        session: Option<SessionId>,
    ) -> RpcResult<PolishReply> {
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            Polish {
                text,
                destination,
                session,
            },
        )
        .await
    }

    async fn rewrite(
        &self,
        instruction: String,
        selection: String,
        app: voice::AppProfile,
        thread: Option<Vec<(String, String)>>,
    ) -> RpcResult<PolishReply> {
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            Rewrite {
                instruction,
                selection,
                app,
                thread,
            },
        )
        .await
    }

    async fn talk_start(
        &self,
        ext: &Extensions,
        session: SessionId,
        sink: talk::Sink,
        sample_rate: Option<u32>,
    ) -> RpcResult<talk::TalkId> {
        let peer = peer(ext)?;
        match (sink, sample_rate) {
            (talk::Sink::Device, _) if peer.remote() => {
                return Err(failed(
                    "speaker and microphone are on the harness host; pass sink: client and sample_rate to stream audio",
                ));
            }
            (talk::Sink::Client, None) => {
                return Err(failed("sink: client needs sample_rate"));
            }
            _ => {}
        }
        let actor = talk()?;
        let id = send(
            &actor,
            talk::StartTalk {
                session,
                sink,
                sample_rate,
            },
        )
        .await?;
        // The talk is bound to the connection that started it (76 §0.10): a
        // dropped connection must not hold the mic, the speaker or the slot.
        tokio::spawn(async move {
            peer.closed().await;
            let _ = send(&actor, talk::StopTalk { id }).await;
        });
        Ok(id)
    }

    async fn talk_stop(&self, ext: &Extensions, id: talk::TalkId) -> RpcResult<()> {
        peer(ext)?;
        send(&talk()?, talk::StopTalk { id }).await
    }

    async fn talk_audio(
        &self,
        ext: &Extensions,
        id: talk::TalkId,
        pcm: String,
        played_seq: Option<u32>,
    ) -> RpcResult<()> {
        peer(ext)?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(pcm)
            .map_err(|e| failed(format!("bad base64: {e}")))?;
        if bytes.len() % 2 != 0 {
            return Err(failed("pcm must be little-endian i16"));
        }
        let pcm = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes(*b))
            .collect();
        send(
            &talk()?,
            talk::PushAudio {
                id,
                pcm,
                played_seq,
            },
        )
        .await
    }

    async fn talk_status(&self, ext: &Extensions) -> RpcResult<talk::TalkState> {
        peer(ext)?;
        send(&talk()?, talk::TalkStatus).await
    }
}

pub(super) trait SpeechConfigExt {
    fn write_voice_config(&self, key: &[&str], value: &str) -> RpcResult<()>;
}

impl SpeechConfigExt for Server {
    /// 153 §14.3: persist `voice.<key>` (one or two levels; the value is YAML flow syntax).
    fn write_voice_config(&self, key: &[&str], value: &str) -> RpcResult<()> {
        let path = self.rt.data_dir.join("config.yaml");
        match key {
            [k] => runtime::write_config_value(&path, "voice", k, value),
            [sub, k] => runtime::write_nested_config_value(&path, "voice", sub, k, value),
            _ => unreachable!("one or two key levels"),
        }
        .map_err(|e| failed(format!("config.yaml: {e}")))
    }

}

/// Build this domain's RPC module.
pub fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(SpeechApiServer::into_rpc(server), rpc_descriptors())
        .expect("talk RPC descriptors")
}
