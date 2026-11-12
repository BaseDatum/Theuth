use super::*;

#[rpc(server, client)]
pub trait MeetingsApi {
    /// Processes Core Audio knows about, the ones playing audio first (53, 01 §4.1):
    /// the picker for `meeting_start`. "meetings are disabled" when the plugin is off.
    #[method(name = "meeting_processes")]
    async fn meeting_processes(&self) -> RpcResult<Vec<meeting::ProcessInfo>>;
    /// Start recording `process` (a pid from `meeting_processes`; `null` = all
    /// system audio); `consent` must be `true`. Replies once the devices are
    /// open, after `Started` and `State{Recording}` went out on
    /// `subscribe_events`. Error strings: consent missing, pid not playing,
    /// "a meeting is already recording", a device or permission failure (the
    /// meeting is then `Failed` in `meeting_list`).
    #[method(name = "meeting_start")]
    async fn meeting_start(&self, start: meeting::StartMeeting) -> RpcResult<meeting::Started>;
    /// Pause the recording: devices stay open, the clock stops, a gap is recorded.
    #[method(name = "meeting_pause")]
    async fn meeting_pause(&self) -> RpcResult<()>;
    /// Resume after `meeting_pause`.
    #[method(name = "meeting_resume")]
    async fn meeting_resume(&self) -> RpcResult<()>;
    /// Stop; replies once the last Ogg pages are written. The post pipeline then
    /// runs in the background (`PostProgress` → `ReviewReady`, 04 §2).
    #[method(name = "meeting_stop")]
    async fn meeting_stop(&self) -> RpcResult<()>;
    /// The current meeting's state, ids, elapsed recording time and degradations.
    #[method(name = "meeting_status")]
    async fn meeting_status(&self) -> RpcResult<meeting::Status>;
    /// Recorded meetings, newest first (`limit` default 100); 158 §9.2: `project` filters
    /// to one project.
    #[method(name = "meeting_list")]
    async fn meeting_list(
        &self,
        limit: Option<u32>,
        project: Option<String>,
    ) -> RpcResult<Vec<meeting::MeetingSummary>>;
    /// One meeting's manifest.
    #[method(name = "meeting_get")]
    async fn meeting_get(&self, id: meeting::MeetingId) -> RpcResult<meeting::Manifest>;
    /// Delete a meeting: directory, index row, session. Refused while it records.
    #[method(name = "meeting_delete")]
    async fn meeting_delete(&self, id: meeting::MeetingId) -> RpcResult<()>;
    /// Microphone and audio-capture TCC state as far as it is known without prompting.
    #[method(name = "meeting_permissions")]
    async fn meeting_permissions(&self) -> RpcResult<meeting::Permissions>;
    /// Phase 2 (02 §6). The user tapped a topic button: `TopicRemoved{Viewed}`,
    /// the stored brief as `Brief{source: Tap}`, a `[meeting tapped]` steer.
    #[method(name = "meeting_tap")]
    async fn meeting_tap(&self, topic: meeting::TopicId) -> RpcResult<()>;
    /// The user dismissed a topic button: `TopicRemoved{Dismissed}`, the label barred, a steer.
    #[method(name = "meeting_dismiss")]
    async fn meeting_dismiss(&self, topic: meeting::TopicId) -> RpcResult<()>;
    /// The user wants more on a brief: a `[meeting deeper]` steer; the answer arrives as `Brief{source: Deeper}`.
    #[method(name = "meeting_deeper")]
    async fn meeting_deeper(&self, brief: meeting::BriefId) -> RpcResult<()>;
    /// A typed question: `[meeting request, typed]` steer; the answer arrives as `Brief{source: Typed}`.
    #[method(name = "meeting_request")]
    async fn meeting_request(&self, text: String) -> RpcResult<()>;
    /// Push-to-talk held/released (53 decision 10): `Listening`; the release tags the
    /// mic utterance as a spoken request and steers it at once.
    #[method(name = "meeting_ptt")]
    async fn meeting_ptt(&self, held: bool) -> RpcResult<()>;
    /// Where the island's cursor is; mirrored to `meeting_status.cursor`.
    #[method(name = "meeting_cursor")]
    async fn meeting_cursor(&self, brief: Option<meeting::BriefId>) -> RpcResult<()>;
    /// Every brief of a meeting, in order (memory while recording, `briefs.jsonl` after).
    #[method(name = "meeting_briefs")]
    async fn meeting_briefs(&self, id: meeting::MeetingId) -> RpcResult<Vec<meeting::Brief>>;
    /// The current meeting's live topic buttons.
    #[method(name = "meeting_topics")]
    async fn meeting_topics(&self) -> RpcResult<Vec<meeting::Topic>>;
    /// A meeting's live transcript (`live.jsonl`, last 2 000 utterances).
    #[method(name = "meeting_live")]
    async fn meeting_live(&self, id: meeting::MeetingId) -> RpcResult<Vec<meeting::Utterance>>;
    /// Phase 4 (04 §5). The transcript of record (`transcript.json`), once the post pipeline is done.
    #[method(name = "meeting_transcript")]
    async fn meeting_transcript(&self, id: meeting::MeetingId) -> RpcResult<meeting::Transcript>;
    /// Apply review edits in order — all or none — and return the new transcript.
    #[method(name = "meeting_edit")]
    async fn meeting_edit(
        &self,
        id: meeting::MeetingId,
        ops: Vec<meeting::EditOp>,
    ) -> RpcResult<meeting::Transcript>;
    /// `[t0, t1)` of one track as a base64 16-bit mono WAV (48 kHz), for the review pane's player.
    #[method(name = "meeting_segment_audio")]
    async fn meeting_segment_audio(
        &self,
        id: meeting::MeetingId,
        track: meeting::Track,
        t0_ms: u64,
        t1_ms: u64,
    ) -> RpcResult<String>;
    /// Enrolled speaker profiles (embedding omitted).
    #[method(name = "meeting_speakers")]
    async fn meeting_speakers(&self) -> RpcResult<Vec<meeting::Profile>>;
    /// 156: a voiceprint from the speech of `segments` (one track): a new profile, or folded into an enrolled one.
    #[method(name = "meeting_speaker_enroll")]
    async fn meeting_speaker_enroll(
        &self,
        id: meeting::MeetingId,
        segments: Vec<u32>,
        target: meeting::EnrollTarget,
    ) -> RpcResult<meeting::Enrolled>;
    /// Delete a speaker profile file. Transcripts keep their labels.
    #[method(name = "meeting_speaker_delete")]
    async fn meeting_speaker_delete(&self, profile: meeting::ProfileId) -> RpcResult<()>;
    /// 07 §6: the `meeting.live_diarization` switch as configured (default `true`).
    #[method(name = "meeting_live_diarization")]
    async fn meeting_live_diarization(&self) -> RpcResult<bool>;
    /// Persist `meeting.live_diarization` and apply it from the next meeting start; a
    /// running meeting keeps its mode. A failed config write changes nothing.
    #[method(name = "set_meeting_live_diarization")]
    async fn set_meeting_live_diarization(&self, on: bool) -> RpcResult<bool>;
    // 158 §15.2 — contract methods (C158). Each body is one call into the owning wave's
    // module; the stubs answer inertly until the wave lands.
    /// 158 §4.5 (W1): the current detection prompt, `null` when none.
    #[method(name = "meeting_detection")]
    async fn meeting_detection(&self) -> RpcResult<Option<meeting::Detection>>;
    /// 158 §4.4 (W1): "Not now" (10-minute cooldown) or, with `never`, "Never for <App>"
    /// (appends to `meeting.detect.ignore`).
    #[method(name = "meeting_detection_dismiss")]
    async fn meeting_detection_dismiss(&self, app_id: String, never: bool) -> RpcResult<()>;
    /// 158 §8.2 (W2): pick (or clear, `null`) the event of the current prompt; the re-rendered prompt.
    #[method(name = "meeting_detection_set_event")]
    async fn meeting_detection_set_event(
        &self,
        event: Option<meeting::EventId>,
    ) -> RpcResult<Option<meeting::Detection>>;
    /// 158 §6.2 (W1): the user-facing `meeting:` block.
    #[method(name = "meeting_settings_get")]
    async fn meeting_settings_get(&self) -> RpcResult<meeting::MeetingSettings>;
    /// 158 §6.2 (W1): persist the whole block to config.yaml and apply it from the next start;
    /// a failed write changes nothing.
    #[method(name = "meeting_settings_set")]
    async fn meeting_settings_set(
        &self,
        settings: meeting::MeetingSettings,
    ) -> RpcResult<meeting::MeetingSettings>;
    /// 158 §9.2 (W3): set or clear a meeting's project (manifest, index, sidebar, memory rows).
    #[method(name = "meeting_set_project")]
    async fn meeting_set_project(
        &self,
        id: meeting::MeetingId,
        project: Option<String>,
    ) -> RpcResult<meeting::Manifest>;
    /// 158 §8.2 (W2): set or clear a meeting's calendar event, any state; a steer while live.
    #[method(name = "meeting_set_event")]
    async fn meeting_set_event(
        &self,
        id: meeting::MeetingId,
        event: Option<meeting::EventId>,
    ) -> RpcResult<meeting::Manifest>;
    /// 158 §11.1 (W4): the note template, changeable until Summarize.
    #[method(name = "meeting_set_template")]
    async fn meeting_set_template(
        &self,
        id: meeting::MeetingId,
        template: String,
    ) -> RpcResult<meeting::Manifest>;
    /// 158 §13 (W6): Review's once-per-meeting "Did you use speakers (not headphones)?".
    #[method(name = "meeting_set_speakers")]
    async fn meeting_set_speakers(
        &self,
        id: meeting::MeetingId,
        speakers: bool,
    ) -> RpcResult<meeting::Manifest>;
    /// 158 §10 (W4): speaker-name suggestions for a reviewed meeting; never applies anything.
    #[method(name = "meeting_suggest_speakers")]
    async fn meeting_suggest_speakers(
        &self,
        id: meeting::MeetingId,
    ) -> RpcResult<Vec<meeting::Suggestion>>;
    /// 158 §11.1 (W4): every template, built-ins first.
    #[method(name = "meeting_templates")]
    async fn meeting_templates(&self) -> RpcResult<Vec<meeting::Template>>;
    /// 158 §11.1 (W4): write `<slug>.md` (markdown + YAML front matter) and return it parsed.
    #[method(name = "meeting_template_put")]
    async fn meeting_template_put(
        &self,
        slug: String,
        body: String,
    ) -> RpcResult<meeting::Template>;
    /// 158 §11.1 (W4): delete a user template; refused for built-ins.
    #[method(name = "meeting_template_delete")]
    async fn meeting_template_delete(&self, slug: String) -> RpcResult<()>;
    /// 158 §11.1 (W4): restore a built-in's shipped body over the user's edits; refused for user templates.
    #[method(name = "meeting_template_reset")]
    async fn meeting_template_reset(&self, slug: String) -> RpcResult<meeting::Template>;
    /// 158 §7.4 (W2): every calendar source.
    #[method(name = "calendar_sources")]
    async fn calendar_sources(&self) -> RpcResult<Vec<meeting::CalSource>>;
    /// 158 §7.3 (W2): add an ICS subscription; the URL goes to the vault as `calendar.ics.<id>`.
    #[method(name = "calendar_source_add")]
    async fn calendar_source_add(&self, name: String, url: String)
    -> RpcResult<meeting::CalSource>;
    /// 158 §7.4 (W2): delete a source and its events (and vault row).
    #[method(name = "calendar_source_delete")]
    async fn calendar_source_delete(&self, id: String) -> RpcResult<()>;
    /// 158 §7.3 (W2): fetch now; the row with `last_fetch` / `last_error`.
    #[method(name = "calendar_source_refresh")]
    async fn calendar_source_refresh(&self, id: String) -> RpcResult<meeting::CalSource>;
    /// 158 §7.2 (W2), companion only (`x-theuth-client: companion`): replace `source`'s rows
    /// inside `window` (RFC 3339 pair) with `events`; the rows written.
    #[method(name = "calendar_push", with_extensions)]
    async fn calendar_push(
        &self,
        source: String,
        window: (String, String),
        events: Vec<meeting::CalEvent>,
    ) -> RpcResult<u32>;
    /// 158 §7.2 (W2), companion only: the EventKit state line (`granted` / `denied` / …).
    #[method(name = "calendar_source_state", with_extensions)]
    async fn calendar_source_state(&self, source: String, state: String) -> RpcResult<()>;
    /// 158 §7.4 (W2): events overlapping `[from, to]` (RFC 3339), start ascending.
    #[method(name = "calendar_events")]
    async fn calendar_events(&self, from: String, to: String) -> RpcResult<Vec<meeting::CalEvent>>;
    /// 158 §7.6 (W2): the `meeting.calendar` block.
    #[method(name = "calendar_settings_get")]
    async fn calendar_settings_get(&self) -> RpcResult<meeting::CalendarConfig>;
    /// 158 §7.6 (W2): persist and apply the `meeting.calendar` block.
    #[method(name = "calendar_settings_set")]
    async fn calendar_settings_set(
        &self,
        config: meeting::CalendarConfig,
    ) -> RpcResult<meeting::CalendarConfig>;
    /// `Review` → `Summarizing`: the guided summary turn; `Summarized{title}` and `State{Done}` follow.
    #[method(name = "meeting_summarize")]
    async fn meeting_summarize(&self, id: meeting::MeetingId) -> RpcResult<()>;
    /// Re-run the post pipeline from the Opus files.
    #[method(name = "meeting_post_retry")]
    async fn meeting_post_retry(&self, id: meeting::MeetingId) -> RpcResult<()>;
    /// `summary.md`.
    #[method(name = "meeting_summary")]
    async fn meeting_summary(&self, id: meeting::MeetingId) -> RpcResult<String>;
    /// `decision_mode("meeting")`.
    #[method(name = "meeting_oracle_mode")]
    async fn meeting_oracle_mode(&self) -> RpcResult<decision::ModeInfo>;
    /// `set_decision_mode("meeting", mode)`.
    #[method(name = "set_meeting_oracle_mode")]
    async fn set_meeting_oracle_mode(&self, mode: decision::Mode) -> RpcResult<decision::ModeInfo>;
    /// Phase 5 (05 §8): the brief cache. Full-text search, best rank first (`limit` ≤ 50, default 8).
    #[method(name = "brief_search")]
    async fn brief_search(
        &self,
        query: String,
        limit: Option<u32>,
    ) -> RpcResult<Vec<meeting::CachedBrief>>;
    /// Cached briefs, newest `updated_at` first, optionally only those tagged `tag` (`limit` ≤ 50).
    #[method(name = "brief_list")]
    async fn brief_list(
        &self,
        tag: Option<String>,
        limit: Option<u32>,
    ) -> RpcResult<Vec<meeting::CachedBrief>>;
    /// Every tag in the cache, most-used first.
    #[method(name = "brief_tags")]
    async fn brief_tags(&self) -> RpcResult<Vec<String>>;
    /// One cached brief.
    #[method(name = "brief_get")]
    async fn brief_get(&self, id: meeting::BriefId) -> RpcResult<meeting::CachedBrief>;
    /// Create (`id` omitted) or update a cached brief; `BriefEvent::Changed` follows. Source `user`.
    #[method(name = "brief_put")]
    async fn brief_put(&self, put: meeting::CachedBriefPut) -> RpcResult<meeting::CachedBrief>;
    /// Delete a cached brief; `BriefEvent::Changed { removed: true }` follows.
    #[method(name = "brief_delete")]
    async fn brief_delete(&self, id: meeting::BriefId) -> RpcResult<()>;
}

#[async_trait]
impl MeetingsApiServer for Server {

    async fn meeting_processes(&self) -> RpcResult<Vec<meeting::ProcessInfo>> {
        send(&meeting()?, meeting::ListProcesses).await
    }

    async fn meeting_start(&self, start: meeting::StartMeeting) -> RpcResult<meeting::Started> {
        send(&meeting()?, start).await
    }

    async fn meeting_pause(&self) -> RpcResult<()> {
        send(&meeting()?, meeting::PauseMeeting).await
    }

    async fn meeting_resume(&self) -> RpcResult<()> {
        send(&meeting()?, meeting::ResumeMeeting).await
    }

    async fn meeting_stop(&self) -> RpcResult<()> {
        send(&meeting()?, meeting::StopMeeting).await
    }

    async fn meeting_status(&self) -> RpcResult<meeting::Status> {
        send(&meeting()?, meeting::MeetingStatus).await
    }

    async fn meeting_list(
        &self,
        limit: Option<u32>,
        project: Option<String>,
    ) -> RpcResult<Vec<meeting::MeetingSummary>> {
        send(&meeting()?, meeting::ListMeetings { limit, project }).await
    }

    async fn meeting_get(&self, id: meeting::MeetingId) -> RpcResult<meeting::Manifest> {
        send(&meeting()?, meeting::GetMeeting { meeting: id }).await
    }

    async fn meeting_delete(&self, id: meeting::MeetingId) -> RpcResult<()> {
        send(&meeting()?, meeting::DeleteMeeting { meeting: id }).await
    }

    async fn meeting_permissions(&self) -> RpcResult<meeting::Permissions> {
        send(&meeting()?, meeting::CheckPermissions).await
    }

    async fn meeting_tap(&self, topic: meeting::TopicId) -> RpcResult<()> {
        send(&meeting()?, meeting::TapTopic { topic }).await
    }

    async fn meeting_dismiss(&self, topic: meeting::TopicId) -> RpcResult<()> {
        send(&meeting()?, meeting::DismissTopic { topic }).await
    }

    async fn meeting_deeper(&self, brief: meeting::BriefId) -> RpcResult<()> {
        send(&meeting()?, meeting::Deeper { brief }).await
    }

    async fn meeting_request(&self, text: String) -> RpcResult<()> {
        send(&meeting()?, meeting::TypedRequest { text }).await
    }

    async fn meeting_ptt(&self, held: bool) -> RpcResult<()> {
        send(&meeting()?, meeting::PushToTalk { held }).await
    }

    async fn meeting_cursor(&self, brief: Option<meeting::BriefId>) -> RpcResult<()> {
        send(&meeting()?, meeting::SetCursor { brief }).await
    }

    async fn meeting_briefs(&self, id: meeting::MeetingId) -> RpcResult<Vec<meeting::Brief>> {
        send(&meeting()?, meeting::ListBriefs { meeting: id }).await
    }

    async fn meeting_topics(&self) -> RpcResult<Vec<meeting::Topic>> {
        send(&meeting()?, meeting::ListTopics).await
    }

    async fn meeting_live(&self, id: meeting::MeetingId) -> RpcResult<Vec<meeting::Utterance>> {
        send(&meeting()?, meeting::LiveText { meeting: id }).await
    }

    async fn meeting_transcript(&self, id: meeting::MeetingId) -> RpcResult<meeting::Transcript> {
        send(&meeting()?, meeting::GetTranscript { meeting: id }).await
    }

    async fn meeting_edit(
        &self,
        id: meeting::MeetingId,
        ops: Vec<meeting::EditOp>,
    ) -> RpcResult<meeting::Transcript> {
        send(&meeting()?, meeting::EditTranscript { meeting: id, ops }).await
    }

    async fn meeting_segment_audio(
        &self,
        id: meeting::MeetingId,
        track: meeting::Track,
        t0_ms: u64,
        t1_ms: u64,
    ) -> RpcResult<String> {
        send(
            &meeting()?,
            meeting::SegmentAudio {
                meeting: id,
                track,
                t0_ms,
                t1_ms,
            },
        )
        .await
    }

    async fn meeting_speakers(&self) -> RpcResult<Vec<meeting::Profile>> {
        send(&meeting()?, meeting::ListSpeakers).await
    }

    async fn meeting_speaker_enroll(
        &self,
        id: meeting::MeetingId,
        segments: Vec<u32>,
        target: meeting::EnrollTarget,
    ) -> RpcResult<meeting::Enrolled> {
        send(
            &meeting()?,
            meeting::EnrollSegments {
                meeting: id,
                segments,
                target,
            },
        )
        .await
    }

    async fn meeting_oracle_mode(&self) -> RpcResult<decision::ModeInfo> {
        decision::rpc_support::RpcDecisionExt::decision_mode(self, meeting::oracle::PREFIX.to_owned()).await
    }

    async fn set_meeting_oracle_mode(&self, mode: decision::Mode) -> RpcResult<decision::ModeInfo> {
        decision::rpc_support::RpcDecisionExt::set_decision_mode(self, meeting::oracle::PREFIX.to_owned(), mode)
            .await
    }

    async fn meeting_speaker_delete(&self, profile: meeting::ProfileId) -> RpcResult<()> {
        send(&meeting()?, meeting::DeleteSpeaker { profile }).await
    }

    async fn meeting_live_diarization(&self) -> RpcResult<bool> {
        send(&meeting()?, meeting::LiveDiarization).await
    }

    async fn set_meeting_live_diarization(&self, on: bool) -> RpcResult<bool> {
        send(&meeting()?, meeting::SetLiveDiarization { on }).await
    }

    // 158 §15.2 — contract methods (C158): one delegate call each, `anyhow` → `ASK_FAILED`.

    async fn meeting_detection(&self) -> RpcResult<Option<meeting::Detection>> {
        meeting::detect::rpc_detection(&self.rt)
            .await
            .map_err(stub_err)
    }

    async fn meeting_detection_dismiss(&self, app_id: String, never: bool) -> RpcResult<()> {
        meeting::detect::rpc_detection_dismiss(&self.rt, app_id, never)
            .await
            .map_err(stub_err)
    }

    async fn meeting_detection_set_event(
        &self,
        event: Option<meeting::EventId>,
    ) -> RpcResult<Option<meeting::Detection>> {
        meeting::calendar::rpc_detection_set_event(&self.rt, event)
            .await
            .map_err(stub_err)
    }

    async fn meeting_settings_get(&self) -> RpcResult<meeting::MeetingSettings> {
        meeting::rpc_settings_get(&self.rt).await.map_err(stub_err)
    }

    async fn meeting_settings_set(
        &self,
        settings: meeting::MeetingSettings,
    ) -> RpcResult<meeting::MeetingSettings> {
        meeting::rpc_settings_set(&self.rt, settings)
            .await
            .map_err(stub_err)
    }

    async fn meeting_set_project(
        &self,
        id: meeting::MeetingId,
        project: Option<String>,
    ) -> RpcResult<meeting::Manifest> {
        meeting::calendar::prebrief::rpc_set_project(&self.rt, id, project)
            .await
            .map_err(stub_err)
    }

    async fn meeting_set_event(
        &self,
        id: meeting::MeetingId,
        event: Option<meeting::EventId>,
    ) -> RpcResult<meeting::Manifest> {
        meeting::calendar::rpc_set_event(&self.rt, id, event)
            .await
            .map_err(stub_err)
    }

    async fn meeting_set_template(
        &self,
        id: meeting::MeetingId,
        template: String,
    ) -> RpcResult<meeting::Manifest> {
        meeting::templates::rpc_set_template(&self.rt, id, template)
            .await
            .map_err(stub_err)
    }

    async fn meeting_set_speakers(
        &self,
        id: meeting::MeetingId,
        speakers: bool,
    ) -> RpcResult<meeting::Manifest> {
        meeting::post::echo::rpc_set_speakers(&self.rt, id, speakers)
            .await
            .map_err(stub_err)
    }

    async fn meeting_suggest_speakers(
        &self,
        id: meeting::MeetingId,
    ) -> RpcResult<Vec<meeting::Suggestion>> {
        meeting::post::suggest::rpc_suggest(&self.rt, id)
            .await
            .map_err(stub_err)
    }

    async fn meeting_templates(&self) -> RpcResult<Vec<meeting::Template>> {
        meeting::templates::rpc_templates(&self.rt)
            .await
            .map_err(stub_err)
    }

    async fn meeting_template_put(
        &self,
        slug: String,
        body: String,
    ) -> RpcResult<meeting::Template> {
        meeting::templates::rpc_template_put(&self.rt, slug, body)
            .await
            .map_err(stub_err)
    }

    async fn meeting_template_delete(&self, slug: String) -> RpcResult<()> {
        meeting::templates::rpc_template_delete(&self.rt, slug)
            .await
            .map_err(stub_err)
    }

    async fn meeting_template_reset(&self, slug: String) -> RpcResult<meeting::Template> {
        meeting::templates::rpc_template_reset(&self.rt, slug)
            .await
            .map_err(stub_err)
    }

    async fn calendar_sources(&self) -> RpcResult<Vec<meeting::CalSource>> {
        meeting::calendar::rpc_sources(&self.rt)
            .await
            .map_err(stub_err)
    }

    async fn calendar_source_add(
        &self,
        name: String,
        url: String,
    ) -> RpcResult<meeting::CalSource> {
        meeting::calendar::rpc_source_add(&self.rt, name, url)
            .await
            .map_err(stub_err)
    }

    async fn calendar_source_delete(&self, id: String) -> RpcResult<()> {
        meeting::calendar::rpc_source_delete(&self.rt, id)
            .await
            .map_err(stub_err)
    }

    async fn calendar_source_refresh(&self, id: String) -> RpcResult<meeting::CalSource> {
        meeting::calendar::rpc_source_refresh(&self.rt, id)
            .await
            .map_err(stub_err)
    }

    async fn calendar_push(
        &self,
        ext: &Extensions,
        source: String,
        window: (String, String),
        events: Vec<meeting::CalEvent>,
    ) -> RpcResult<u32> {
        meeting::calendar::rpc_push(&self.rt, client_of(ext), source, window, events)
            .await
            .map_err(stub_err)
    }

    async fn calendar_source_state(
        &self,
        ext: &Extensions,
        source: String,
        state: String,
    ) -> RpcResult<()> {
        meeting::calendar::rpc_source_state(&self.rt, client_of(ext), source, state)
            .await
            .map_err(stub_err)
    }

    async fn calendar_events(&self, from: String, to: String) -> RpcResult<Vec<meeting::CalEvent>> {
        meeting::calendar::rpc_events(&self.rt, from, to)
            .await
            .map_err(stub_err)
    }

    async fn calendar_settings_get(&self) -> RpcResult<meeting::CalendarConfig> {
        meeting::calendar::rpc_settings_get(&self.rt)
            .await
            .map_err(stub_err)
    }

    async fn calendar_settings_set(
        &self,
        config: meeting::CalendarConfig,
    ) -> RpcResult<meeting::CalendarConfig> {
        meeting::calendar::rpc_settings_set(&self.rt, config)
            .await
            .map_err(stub_err)
    }

    async fn meeting_summarize(&self, id: meeting::MeetingId) -> RpcResult<()> {
        send(&meeting()?, meeting::Summarize { meeting: id }).await
    }

    async fn meeting_post_retry(&self, id: meeting::MeetingId) -> RpcResult<()> {
        send(&meeting()?, meeting::PostRetry { meeting: id }).await
    }

    async fn meeting_summary(&self, id: meeting::MeetingId) -> RpcResult<String> {
        send(&meeting()?, meeting::GetSummary { meeting: id }).await
    }

    async fn brief_search(
        &self,
        query: String,
        limit: Option<u32>,
    ) -> RpcResult<Vec<meeting::CachedBrief>> {
        send(&meeting()?, meeting::BriefSearch { query, limit }).await
    }

    async fn brief_list(
        &self,
        tag: Option<String>,
        limit: Option<u32>,
    ) -> RpcResult<Vec<meeting::CachedBrief>> {
        send(&meeting()?, meeting::BriefList { tag, limit }).await
    }

    async fn brief_tags(&self) -> RpcResult<Vec<String>> {
        send(&meeting()?, meeting::BriefTags).await
    }

    async fn brief_get(&self, id: meeting::BriefId) -> RpcResult<meeting::CachedBrief> {
        send(&meeting()?, meeting::BriefGet { id }).await
    }

    async fn brief_put(&self, put: meeting::CachedBriefPut) -> RpcResult<meeting::CachedBrief> {
        send(
            &meeting()?,
            meeting::BriefPut {
                put,
                source: "user".into(),
            },
        )
        .await
        .map(|(row, _)| row)
    }

    async fn brief_delete(&self, id: meeting::BriefId) -> RpcResult<()> {
        send(&meeting()?, meeting::BriefDelete { id }).await
    }
}
