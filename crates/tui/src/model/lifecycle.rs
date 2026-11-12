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
            skills: std::mem::take(&mut self.skills),
            width: self.width,
            components: std::mem::replace(&mut self.components, crate::components::Registry::empty()),
            component_commands: self.component_commands.take(),
            todo_open: std::mem::take(&mut self.todo_open),
            // Per session: the switch view belongs to whatever is shown next
            // (the global default after `/new`).
            refresh_communication: true,
            app_enabled: self.app_enabled,
            advisor_default: std::mem::take(&mut self.advisor_default),
            vision_model: self.vision_model.take(),
            search: std::mem::take(&mut self.search),
            private: self.private.take(),
            private_warned: self.private_warned,
            dictation: self.dictation.take(),
            dictation_pending: self.dictation_pending.take(),
            dictation_model: self.dictation_model.clone(),
            attached: self.attached.take(),
            model_id: self.default_model.clone(),
            default_model: std::mem::take(&mut self.default_model),
            approvals_global: self.approvals_global,
            ..Self::default()
        };
    }

    /// Show a loaded transcript: `UiEvent::History` and `UiEvent::Forked`.
    pub(super) fn load(&mut self, loaded: Loaded) {
        self.component_notify(crate::components::Lifecycle::TranscriptReplaced);
        self.component_notify(crate::components::Lifecycle::History(&loaded));
        let opts = self.assistant_options();
        self.entries = loaded
            .history
            .into_iter()
            .flat_map(|h| from_history(h, &opts))
            .collect();
        self.refresh_artifacts = self
            .entries
            .iter()
            .any(|e| matches!(e, Entry::Tool { name, .. } if name == SHOW || name == APP));
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
        self.loaded_skills = loaded.skills;
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
        // The link survives a restart (125 §3): re-read it with the transcript.
        self.refresh_advisor = true;
        if let Some(n) = loaded.context_estimate {
            self.context = n;
            self.context_estimated = true;
        }
        self.loaded_todos(loaded.todos);
        // The load carries the switches too; a `get_communication` reply
        // already in flight is reconciled by revision.
        if let Some(c) = loaded.communication {
            self.set_communication(c);
        }
    }
}

impl Model {

    /// A settled reload (`UiEvent::Refreshed`, 48 §5): shown only when the
    /// actor read it idle, nothing started streaming here meanwhile, and its
    /// revision is not behind the one on screen. Otherwise the transcript
    /// stays as it is, marked stale until the next settled notification;
    /// a pending notice (our own removal) lands anyway.
    pub(super) fn refresh(&mut self, mut loaded: Loaded) {
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
            self.loaded_todos(loaded.todos.take());
            return;
        }
        if !loaded.settled || self.streaming || theirs < ours {
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
            self.loaded_todos(loaded.todos.take());
            return;
        }
        // A picker over the old inventory would be refused as stale anyway.
        if matches!(self.menu, Some(Menu::Shake(_))) {
            self.menu = None;
            self.entries.push(Entry::Info(
                "context changed — /shake again to review".into(),
            ));
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
