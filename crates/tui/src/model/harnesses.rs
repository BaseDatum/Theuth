use super::*;

/// One line of the `/acp` menu.
#[derive(Debug)]
pub enum AcpRow<'a> {
    /// A configured harness (`agents.yaml`).
    Entry(&'a EntryInfo),
    /// Blank line between the two halves; the cursor skips it.
    Sep,
    /// A registry agent, addable with Enter.
    Registry(&'a RegistryAgent),
}

impl Model {

    /// `/acp` menu rows in cursor order: entries, a separator when both
    /// halves are present, then the registry.
    pub fn acp_rows(&self) -> Vec<AcpRow<'_>> {
        let Some(Menu::Acp {
            entries, registry, ..
        }) = &self.menu
        else {
            return Vec::new();
        };
        let mut rows: Vec<AcpRow<'_>> = entries.iter().map(AcpRow::Entry).collect();
        if !entries.is_empty() && !registry.is_empty() {
            rows.push(AcpRow::Sep);
        }
        rows.extend(registry.iter().map(AcpRow::Registry));
        rows
    }

    pub(super) fn acp_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        enum Act {
            Move(usize),
            Send(Box<Command>),
            Probe,
            Close,
            Nothing,
        }
        let Some(Menu::Acp { cursor, .. }) = &self.menu else {
            return MenuKey::Consumed;
        };
        let rows = self.acp_rows();
        let last = rows.len().saturating_sub(1);
        let cursor = (*cursor).min(last);
        // Up/Down step over the separator row.
        let step = |to: usize, dir: isize| match rows.get(to) {
            Some(AcpRow::Sep) => to.saturating_add_signed(dir).min(last),
            _ => to,
        };
        let act = match (code, rows.get(cursor)) {
            (KeyCode::Up, _) => Act::Move(step(cursor.saturating_sub(1), -1)),
            (KeyCode::Down, _) => Act::Move(step((cursor + 1).min(last), 1)),
            (KeyCode::Esc, _) => Act::Close,
            (KeyCode::Char('p'), _) => Act::Probe,
            (KeyCode::Enter, Some(AcpRow::Registry(a))) => {
                Act::Send(Box::new(Command::AddAcp(a.id.clone())))
            }
            (KeyCode::Char('d') | KeyCode::Backspace, Some(AcpRow::Entry(e))) => {
                Act::Send(Box::new(Command::RemoveAcp(e.name.clone())))
            }
            _ => Act::Nothing,
        };
        let Some(Menu::Acp {
            entries,
            cursor: cur,
            ..
        }) = &mut self.menu
        else {
            return MenuKey::Consumed;
        };
        *cur = cursor;
        match act {
            Act::Move(to) => *cur = to,
            Act::Probe => {
                for e in entries.iter_mut() {
                    e.status = Some("probing…".into());
                }
                send(tx, Command::ListAcp { probe: true });
            }
            Act::Send(cmd) => send(tx, *cmd),
            Act::Close => self.menu = None,
            Act::Nothing => {}
        }
        MenuKey::Consumed
    }

    /// Bare `/model` / `/effort` on an ACP session (75 §8, 194 D6): the
    /// harness picker — the `model` select's values as the rows, the
    /// `thought_level` values as the chips under the current one (no
    /// Default). A harness offering neither lists its options as lines;
    /// `/effort` without a `thought_level` option is a notice.
    pub(super) fn open_harness(&mut self, options: Vec<acp::ConfigOption>) {
        let chips = std::mem::take(&mut self.pick_effort);
        let agent = self.agent.clone().unwrap_or_else(|| "harness".into());
        match harness_groups(&options) {
            (None, None) => {
                for line in harness_options_lines(&options) {
                    self.entries.push(Entry::Info(line));
                }
            }
            (_, None) if chips => self.entries.push(Entry::Info(format!(
                "{agent} has no reasoning effort levels"
            ))),
            _ => {
                let items = harness_models(&options, &agent, &self.model_id);
                self.open_models(items, PickTarget::Harness, options, chips);
            }
        }
    }
}

/// `/model` / `/effort` on an ACP session (75 §8): one line per harness
/// option — `model: anthropic/claude-opus-4-8  (has: a, b, c)` — the current
/// value first, then its values verbatim; the `thought_level` option is
/// labelled `effort` so `/effort <value>` reads back.
pub fn harness_options_lines(options: &[acp::ConfigOption]) -> Vec<String> {
    if options.is_empty() {
        return vec!["harness offers no options".into()];
    }
    options
        .iter()
        .map(|o| {
            let label = match o.category.as_deref() {
                Some("thought_level") => "effort",
                _ => o.id.as_str(),
            };
            let values: Vec<&str> = o.options.iter().map(|v| v.value.as_str()).collect();
            format!("{label}: {}  (has: {})", o.current, values.join(", "))
        })
        .collect()
}

/// The harness's `model` select and `thought_level` option, when offered
/// (the desktop's `harnessGroups`).
pub(super) fn harness_groups(
    options: &[acp::ConfigOption],
) -> (Option<&acp::ConfigOption>, Option<&acp::ConfigOption>) {
    let named = |s: &str| s.to_lowercase().contains("model");
    let model = options
        .iter()
        .find(|o| o.category.as_deref() == Some("model"))
        .or_else(|| options.iter().find(|o| named(&o.id) || named(&o.name)));
    let effort = options
        .iter()
        .find(|o| o.category.as_deref() == Some("thought_level"));
    (model, effort)
}

/// The harness picker's rows (the desktop's `HarnessPicker`): one per
/// `model` value, its provider the value's `provider/` prefix else `agent`;
/// without a model select one row for what it runs (`running`, else
/// `agent`). Every row carries the `thought_level` values as its efforts.
pub(super) fn harness_models(
    options: &[acp::ConfigOption],
    agent: &str,
    running: &str,
) -> Vec<ModelInfo> {
    let (model, effort) = harness_groups(options);
    let efforts: Vec<String> = effort
        .map(|o| o.options.iter().map(|v| v.value.clone()).collect())
        .unwrap_or_default();
    let row = |id: &str, name: &str| ModelInfo {
        id: id.to_owned(),
        name: name.to_owned(),
        provider: id.split_once('/').map_or(agent, |(p, _)| p).to_owned(),
        context: 0,
        reasoning: !efforts.is_empty(),
        image: false,
        efforts: efforts.clone(),
        budget_min: None,
        budget_max: None,
        local: false,
    };
    match model {
        Some(o) => o.options.iter().map(|v| row(&v.value, &v.name)).collect(),
        None => {
            let id = if running.is_empty() { agent } else { running };
            vec![row(id, id)]
        }
    }
}

/// The announcement after `set_harness_option` (75 §8): `model → x`, or
/// `model → x after this turn` when the change waits for the turn in flight.
pub fn harness_option_set_line(key: &str, value: &str, r: &acp::SetOptionResult) -> String {
    if r.pending {
        format!("{key} → {value} after this turn")
    } else {
        format!("{key} → {value}")
    }
}
