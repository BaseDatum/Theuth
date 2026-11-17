use super::*;

impl Model {
    /// Fresh state showing `default_model` in the header.
    pub fn new(default_model: String, approvals: bool) -> Self {
        Self::with_components(default_model, approvals, crate::components::builtin())
    }

    /// Fresh core state with a boot-configured registry, without rebuilding its owners.
    pub fn with_components(default_model: String, approvals: bool, components: crate::components::Registry) -> Self {
        Self {
            model_id: default_model.clone(),
            default_model,
            approvals_global: approvals,
            components,
            ..Self::default()
        }
    }

    /// `Self::new` on the engine's default model and effort, keeping catalog and skill index: `/new` and `/resume`.
    pub(super) fn reset(&mut self) {
        self.component_notify(crate::components::Lifecycle::SessionReset);
        *self = Self {
            effort: self.default_effort.clone(),
            default_effort: self.default_effort.take(),
            catalog: self.catalog.take(),
            width: self.width,
            terminal_height: self.terminal_height,
            viewport: self.viewport,
            components: std::mem::take(&mut self.components),
            component_commands: self.component_commands.take(),

            private: self.private.take(),
            private_warned: self.private_warned,
            attached: self.attached.take(),
            model_id: self.default_model.clone(),
            default_model: std::mem::take(&mut self.default_model),
            approvals_global: self.approvals_global,
            ..Self::default()
        };
    }

    /// Present owner-built history without changing context or losing active prompts.
    pub(super) fn present_transcript(&mut self, build: impl FnOnce(&mut crate::components::Registry) -> (crate::components::TranscriptPresentation, crate::components::LocalReply)) -> crate::components::LocalReply {
        let previous_session = self.session;
        let approvals = std::mem::take(&mut self.approvals);
        let asks = std::mem::take(&mut self.asks);
        self.reset();
        self.approvals = approvals;
        self.asks = asks;
        let (frame, reply) = build(&mut self.components);
        self.session = Some(frame.session);
        self.entries = frame.entries;
        self.anchor = frame.anchor;
        self.historical = frame.read_only;
        self.model_id = frame.loaded.model;
        self.harness_model = frame.loaded.harness_model;
        self.harness_commands = frame.loaded.harness_commands;
        self.context_size = frame.loaded.context_size;
        self.agent = frame.loaded.agent;
        self.effort = frame.loaded.effort;
        self.session_name = frame.loaded.name;
        self.session_parent = frame.loaded.parent;
        self.streaming = frame.loaded.running;
        if self.session != previous_session {
            self.component_notify(crate::components::Lifecycle::SessionChanged);
        }
        reply
    }

    /// Show a loaded transcript: `UiEvent::History` and `UiEvent::Forked`.
    pub(super) fn load(&mut self, loaded: Loaded) {
        self.component_notify(crate::components::Lifecycle::SessionRead(&loaded));
        self.component_notify(crate::components::Lifecycle::TranscriptReplaced);
        self.component_notify(crate::components::Lifecycle::History(&loaded));
        let opts = self.assistant_options();
        self.entries = loaded
            .history
            .into_iter()
            .flat_map(|h| from_history(h, &opts, &mut self.components))
            .collect();
        self.anchor = None;
        // An index into the old transcript is meaningless now.
        self.editing = None;
        // The reload an undo/redo triggered shows its own notice
        // instead of the generic depth line.
        match self.notice.take() {
            Some(n) => self.entries.push(Entry::Info(n)),
            None if loaded.undone > 0 => self.entries.push(Entry::Info(format!(
                "{} turn(s) undone — /redo restores",
                loaded.undone
            ))),
            None => {}
        }
        self.queued = loaded.queued;
        self.model_id = loaded.model;
        self.harness_model = loaded.harness_model;
        self.harness_commands = loaded.harness_commands;
        self.context_size = loaded.context_size;
        self.agent = loaded.agent;
        // 141 §10.1: `observed` is set iff the row is watch-only (`kind` alone
        // is `chat` from an older engine), so it is the one flag to keep.
        self.observed = loaded.observed;
        self.effort = loaded.effort;
        self.streaming = loaded.running;
        self.session_name = loaded.name;
        self.session_parent = loaded.parent;
        self.context_revision = loaded.context_revision;
        self.context_editable = loaded.context_editable;
        self.context_reason = loaded.context_reason;
        self.context_stale = false;
        self.project = loaded.project;
        // 139 §9.3: a turn open on another engine (`Loaded.resume.here = false`).
        self.owner_turn = loaded
            .resume
            .filter(|r| !r.here)
            .map(|r| r.owner.unwrap_or_default());
        if let Some(n) = loaded.context_estimate {
            self.context = n;
            self.context_estimated = true;
        }
    }
}

impl Model {

    /// A settled reload (`UiEvent::Refreshed`, 48 §5): shown only when the
    /// actor read it idle, nothing started streaming here meanwhile, and its
    /// revision is not behind the one on screen. Otherwise the transcript
    /// stays as it is, marked stale until the next settled notification;
    /// a pending notice (our own removal) lands anyway.
    pub(super) fn refresh(&mut self, loaded: Loaded) {
        let (theirs, ours) = (
            revision(&loaded.context_revision),
            revision(&self.context_revision),
        );
        // Our own removal reloads once through `RemoveContext` and once more
        // when its `ContextChanged` arrives first: the second is the same picture.
        if loaded.settled
            && theirs == ours
            && ours != 0
            && !self.context_stale
            && self.notice.is_none()
        {
            self.component_notify(crate::components::Lifecycle::SessionRead(&loaded));
            return;
        }
        if !loaded.settled || self.streaming || theirs < ours {
            self.component_notify(crate::components::Lifecycle::SessionRead(&loaded));
            self.context_stale = true;
            self.context_editable = false;
            if let Some(n) = self.notice.take() {
                self.entries.push(Entry::Info(n));
            }
            if !loaded.settled
                && let Some(reason) = loaded.context_reason
            {
                self.entries.push(Entry::Info(format!(
                    "{reason} — transcript refreshes when it settles"
                )));
            }
            return;
        }
        self.load(loaded);
    }

    /// Markdown options for assistant text: the terminal width less the
    /// card padding, `surface::fence` for `mermaid`/`json-render` fences.
    pub(super) fn assistant_options(&self) -> markdown::Options {
        markdown_options(self.width)
    }
}

/// Assistant/card markdown options at the accepted terminal width.
pub(crate) fn markdown_options(width: usize) -> markdown::Options {
    markdown::Options { width: if width == 0 { 0 } else { width.saturating_sub(2).max(20) }, fence: Some(FENCE.clone()) }
}

/// The notice after `remove_context`: what left the context and the new
/// estimate — an estimate, not provider usage (48 §3.2).
pub fn removal_line(r: &ContextRemoval) -> String {
    let mut parts = Vec::new();
    for (n, noun) in [
        (r.messages, "message(s)"),
        (r.images, "image(s)"),
        (r.documents, "document(s)"),
        (r.surfaces, "surface(s)"),
        (r.heads, "summary head(s)"),
    ] {
        if n > 0 {
            parts.push(format!("{n} {noun}"));
        }
    }
    if parts.is_empty() {
        return "nothing removed: already outside the context".into();
    }
    format!(
        "removed from context: {} · ~{} tokens now · originals stay in history",
        parts.join(", "),
        k(r.estimated_tokens)
    )
}
