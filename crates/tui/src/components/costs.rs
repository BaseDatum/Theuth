//! Authoritative owner cost summaries; no local usage summation.
use agent::{SessionCost, CostBucket, CostScope, Coverage, Money};
use rpc::api::clients::SessionsApiClient;
use crate::model::{k, short};
use super::*;

#[derive(Default)]
pub struct Costs {
    pub pending: bool,
    pub cost: Option<SessionCost>,
    menu: Option<(SessionCost, usize)>,
    refresh: bool,
}
#[derive(Clone)]
pub enum Request { SessionCost(SessionId) }
pub enum Msg {
    Loaded(SessionId, Result<SessionCost, String>, bool),
    Changed,
}
impl Costs {
    fn request(&self, session: SessionId, print: bool, cx: &mut Cx<'_, '_, Msg>) {
        cx.spawn_request(Lane::Concurrent, Request::SessionCost(session), move |api, Request::SessionCost(session)| async move {
            Msg::Loaded(session, api.session_cost(session).await.map_err(crate::reason), print)
        });
    }
    /// `/cost` rows (51 §8.3): direct, descendants and total buckets, the
    /// estimate line, unresolved attempts, history coverage, remote-meter
    /// gaps, scope, then one row per immediate child.
    pub fn cost_rows(&self) -> Vec<String> {
        let Some((c, _)) = &self.menu else {
            return Vec::new();
        };
        let bucket = |name: &str, b: &CostBucket| {
            let mut lines = cost_lines(b);
            lines[0] = format!("{name:<12}{}", lines[0]);
            for l in &mut lines[1..] {
                *l = format!("{:<12}{l}", "");
            }
            lines
        };
        let mut rows = bucket("direct", &c.direct);
        rows.extend(bucket("descendants", &c.descendants));
        rows.extend(bucket("total", &c.total));
        rows.push(format!(
            "{:<12}{} calls · {} dispatched · {} unavailable · {} unresolved",
            "attempts", c.total.calls, c.total.dispatched, c.total.unavailable, c.total.unresolved
        ));
        rows.push(format!(
            "{:<12}in {} · out {} · cache read {} · cache write {}{}",
            "tokens",
            k(u32::try_from(c.total.tokens.input).unwrap_or(u32::MAX)),
            k(u32::try_from(c.total.tokens.output).unwrap_or(u32::MAX)),
            k(u32::try_from(c.total.tokens.cache_read).unwrap_or(u32::MAX)),
            k(u32::try_from(c.total.tokens.cache_write).unwrap_or(u32::MAX)),
            match c.total.tokens.unknown_calls {
                0 => String::new(),
                n => format!(" · {n} calls without token counts"),
            }
        ));
        // 75 §7 / 06 §8.4: ACP children's tokens per harness model; a
        // `lower_bound` group (opencode 1.x, #1860) says so.
        for r in &c.remote_tokens {
            rows.push(format!(
                "{:<12}{} · {} turn(s) · in {} · out {} · cache read {} · cache write {}{}",
                "",
                r.model.as_deref().unwrap_or(&r.harness),
                r.turns,
                k(u32::try_from(r.tokens.input).unwrap_or(u32::MAX)),
                k(u32::try_from(r.tokens.output).unwrap_or(u32::MAX)),
                k(u32::try_from(r.tokens.cache_read).unwrap_or(u32::MAX)),
                k(u32::try_from(r.tokens.cache_write).unwrap_or(u32::MAX)),
                if r.lower_bound {
                    " · last request only"
                } else {
                    ""
                }
            ));
        }
        rows.push(format!(
            "{:<12}{}",
            "history",
            match c.history_coverage {
                Coverage::Complete => "complete",
                Coverage::Unknown =>
                    "historical cost unknown (ledger starts after this session did)",
                Coverage::Ambiguous => "ambiguous",
            }
        ));
        if c.remote_meter_gaps > 0 {
            rows.push(format!(
                "{:<12}{} remote meter(s) with incomplete coverage",
                "remote", c.remote_meter_gaps
            ));
        }
        if c.delta_suspected {
            rows.push(format!(
                "{:<12}harness may report per-turn cost — totals may be low",
                "remote"
            ));
        }
        rows.push(format!(
            "{:<12}{}",
            "scope",
            match c.scope {
                CostScope::Native => "native",
                CostScope::OpaqueRemoteInclusive => "includes hidden remote descendants",
                // 141 §7.6: the harness's own total excluded this subagent.
                CostScope::Observed => "watch-only subagent, counted beside the harness",
            }
        ));
        for ch in &c.children {
            rows.push(format!(
                "  ↳ {} · {}{}{}",
                ch.name.as_deref().unwrap_or("(unnamed)"),
                cost_short(&ch.total),
                if ch.deleted { " · deleted" } else { "" },
                match ch.scope {
                    CostScope::Native => "",
                    CostScope::OpaqueRemoteInclusive => " · remote inclusive",
                    CostScope::Observed => " · watch-only",
                }
            ));
        }
        rows
    }

}
impl Component for Costs {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration { id: "costs", interests: &[Interest { topic: "agent.events", version: 1, variants: &["ModelUsageChanged"] }], slash: &[("/cost", "", "model cost of this session's tree: returned, estimated, coverage, per child")], bindings: &[] }
    }
    fn decode(&self, event: &TopicEvent, core: &CoreView<'_>) -> Option<Msg> {
        if event.topic().is_some() && (event.topic() != Some("agent.events") || event.version() != Some(1)) { return None; }
        let agent::AgentEvent::ModelUsageChanged { session, .. } = core.agent? else { return None };
        (core.session == Some(*session) || core.parent == Some(*session) || core.private_root == Some(*session)).then_some(Msg::Changed)
    }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        match input {
            Input::Slash { .. } => match cx.core.session {
                Some(session) => { self.pending = true; self.request(session, true, cx); }
                None => cx.core(CoreAction::Notice("no session".into())),
            },
            Input::Message(Msg::Changed) => self.refresh = true,
            Input::Message(Msg::Loaded(session, result, print)) => {
                match result {
                    Ok(cost) => {
                        if print {
                            if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
                            for line in cost_lines(&cost.total) { cx.plain(Plain::Line(line)); }
                        }
                        if cx.core.session != Some(session) { return; }
                        if let Some((shown, _)) = &mut self.menu { *shown = cost.clone(); }
                        else if std::mem::take(&mut self.pending) {
                            self.menu = Some((cost.clone(), 0));
                            cx.focus(FocusClass::Modal);
                        }
                        self.cost = Some(cost);
                    }
                    Err(error) => cx.error(error),
                }
            }
            Input::Lifecycle(Lifecycle::CommandFailed(_)) => self.pending = false,
            Input::Lifecycle(Lifecycle::FocusLost) => self.menu = None,
            Input::Lifecycle(Lifecycle::SessionReset) => { self.pending = false; self.cost = None; self.menu = None; self.refresh = false; }
            Input::Lifecycle(Lifecycle::SessionChanged | Lifecycle::History(_) | Lifecycle::Resync) => self.refresh = true,
            Input::Lifecycle(Lifecycle::EngineChanged) => self.refresh = cx.core.session.is_some(),
            Input::Tick { .. } if std::mem::take(&mut self.refresh) => {
                if cx.core.interactive && let Some(session) = cx.core.session { self.request(session, false, cx); }
            }
            Input::Key(key) if self.menu.is_some() => {
                let rows = self.cost_rows().len();
                let (_, cursor) = self.menu.as_mut().unwrap();
                match key.code {
                    KeyCode::Up => *cursor = cursor.saturating_sub(1),
                    KeyCode::Down => *cursor = (*cursor + 1).min(rows.saturating_sub(1)),
                    KeyCode::Esc => { self.menu = None; cx.close(); }
                    _ => {}
                }
                cx.consume_key();
            }
            _ => {}
        }
    }
    fn view(&self, at: Surface<'_>, cx: &ViewCx<'_>) -> Option<View> {
        match at {
            Surface::Header(HeaderSlot::Costs) => self.cost.as_ref().map(|c| View::Text(format!("  ·  cost {}", cost_short(&c.total)))),
            Surface::SessionBadge(session) => self.cost.as_ref()?.children.iter().find(|ch| ch.session == session).map(|ch| View::Text(format!("  ·  {}", cost_short(&ch.total)))),
            Surface::Modal => {
                let (cost, cursor) = self.menu.as_ref()?;
                let title = format!("cost · {} · rev {}   esc close   (owner summary; nothing is summed here)", short(cost.session), cost.revision);
                let rows = self.cost_rows();
                Some(View::Element { rows: rows.len().min(cx.height.saturating_sub(10).max(10)) + 3, replace: true,
                    element: crate::ui::menu_view(title, rows, *cursor, cx.width as u16, cx.height as u16, true) })
            }
            _ => None,
        }
    }
    fn wake(&self) -> Option<u64> { self.refresh.then_some(0) }
}
/// Nanos as units with three decimals, rounded half up: `0.123`. Integer
/// arithmetic only (51 §6.2: nanos never go through a float).
pub fn money_units(nanos: u64) -> String {
    let millis = (nanos / 1_000_000) + u64::from(nanos % 1_000_000 >= 500_000);
    format!("{}.{:03}", millis / 1000, millis % 1000)
}

/// The display symbol for an ISO 4217 code, from `iso_currency`'s table, or `None`
/// to print the code. The table gives 18 currencies a bare `$` (and several a `£`,
/// `¥`, `kr`), so a symbol is used only when no other currency shares it, or for
/// the three CLDR English also shows bare (USD `$`, GBP `£`, JPY `¥`) — the same
/// result the desktop gets from `Intl` for these, minus its `CA$`-style prefixes.
fn currency_symbol(code: &str) -> Option<String> {
    use iso_currency::{Currency, IntoEnumIterator};
    let c = Currency::from_code(code)?;
    let sym = c.symbol().symbol;
    if sym.is_empty() || sym == "¤" || sym == code {
        return None;
    }
    let shared = Currency::iter()
        .filter(|o| o.symbol().symbol == sym)
        .count()
        > 1;
    (!shared || matches!(code, "USD" | "GBP" | "JPY")).then_some(sym)
}

/// `$0.123 · €0.010 · CAD 0.500`: one entry per currency, as sorted by the owner.
fn money_list(list: &[Money], prefix: &str) -> String {
    list.iter()
        .map(|m| money(m, prefix))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// `$0.123`, else `CAD 0.500`, after `prefix`.
fn money(m: &Money, prefix: &str) -> String {
    match currency_symbol(&m.currency) {
        Some(s) => format!("{prefix}{s}{}", money_units(m.nanos)),
        None => format!("{prefix}{} {}", m.currency, money_units(m.nanos)),
    }
}

/// The display rules of 51 §6.2 over one bucket: `Returned cost —` when
/// nothing was dispatched; `Returned cost $0.123`; partial coverage adds
/// `· N unavailable`; calls the provider did not price but the catalog or
/// adapter could add a second line `Estimated ~$0.023`. Returned and
/// estimated never share a line.
pub fn cost_lines(b: &CostBucket) -> Vec<String> {
    let mut first = String::from("Returned cost ");
    if b.dispatched == 0 || b.returned.is_empty() {
        first.push('—');
    } else {
        first.push_str(&money_list(&b.returned, ""));
    }
    if b.unavailable > 0 {
        first.push_str(&format!(" · {} unavailable", b.unavailable));
    }
    let mut lines = vec![first];
    if !b.estimated_for_unavailable.is_empty() {
        lines.push(format!(
            "Estimated {}",
            money_list(&b.estimated_for_unavailable, "~")
        ));
    }
    lines
}

/// One bucket for a header tag or child row: returned money, then the
/// estimate as `~$0.123` (the tilde keeps its provenance visible), joined
/// ` · `; else `—`. A known-zero returned entry (an omp child's meter before
/// it reports) is dropped when an estimate or unavailable calls exist, so it
/// never hides them.
pub fn cost_short(b: &CostBucket) -> String {
    let hide_zero = !b.estimated_for_unavailable.is_empty() || b.unavailable > 0;
    let terms: Vec<String> = b
        .returned
        .iter()
        .filter(|m| !(hide_zero && m.nanos == 0))
        .map(|m| money(m, ""))
        .chain(b.estimated_for_unavailable.iter().map(|m| money(m, "~")))
        .collect();
    if terms.is_empty() {
        "—".into()
    } else {
        terms.join(" · ")
    }
}


#[cfg(test)]
#[test]
fn late_cost_error_keeps_current_summary_and_global_error_effects() {
    use crate::model::{Entry, Model, UiEvent};
    let a = SessionId::new();
    let b = SessionId::new();
    let mut model = Model::new(String::new(), false);
    model.session = Some(b);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    model.mount_components(&tx);
    let token = model.components.token("costs");
    let cost = SessionCost {
        session: b, revision: 7, history_coverage: Coverage::Complete, scope: CostScope::Native,
        direct: CostBucket::default(), descendants: CostBucket::default(), total: CostBucket::default(),
        children: vec![], remote_meter_gaps: 0, delta_suspected: false, remote_tokens: vec![],
    };
    model.apply(UiEvent::Component(LocalReply { to: token, engine_epoch: 0, body: Box::new(Msg::Loaded(b, Ok(cost), false)) }));
    model.submit("/cost", &tx);
    model.compacting = true;
    assert!(model.components.get::<Costs>().unwrap().pending);
    model.apply(UiEvent::Component(LocalReply { to: token, engine_epoch: 0, body: Box::new(Msg::Loaded(a, Err("old request failed".into()), true)) }));
    let costs = model.components.get::<Costs>().unwrap();
    assert_eq!(model.session, Some(b));
    assert_eq!(costs.cost.as_ref().map(|c| (c.session, c.revision)), Some((b, 7)));
    assert!(!costs.pending);
    assert!(!model.compacting);
    assert!(matches!(model.entries.last(), Some(Entry::Error(error)) if error == "old request failed"));
}
