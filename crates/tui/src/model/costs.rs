use super::*;

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

impl Model {

    /// `  ·  cost $0.123` (`$0.123 · ~$0.023` with an estimate, `~$0.023` alone) for
    /// the shown session's inclusive total; empty until the owner summary arrived (51 §8.3).
    pub(super) fn cost_tag(&self) -> String {
        match &self.cost {
            Some(c) => format!("  ·  cost {}", cost_short(&c.total)),
            None => String::new(),
        }
    }

    /// One `/agents` picker or board line: [`agent_row`] plus the child's
    /// cost tag (141 R3 T13: watch rows carry their money like the desktop).
    /// The tag is measured first and the row fitted to the rest (R4 T13b), so
    /// the detail is what gets cut, never the money.
    pub fn board_row(&self, a: &AgentRow, width: u16) -> String {
        let tag = self.child_cost_tag(a.info.id);
        let fit =
            width.saturating_sub(u16::try_from(crate::search::cells(&tag)).unwrap_or(u16::MAX));
        format!("{}{tag}", agent_row(a, self.spinner(), fit))
    }

    /// A direct child's inclusive total for its `/agents` row: `  ·  $0.123`
    /// (`~$` for an estimate, `—` when nothing was priced); empty for deeper rows and before the summary arrived.
    // ponytail: `session_cost` is fetched for the shown session only, so a grandchild (a watch row under a
    // harness that is not the shown session) has no figure here; fetch per row if that matters.
    pub fn child_cost_tag(&self, id: SessionId) -> String {
        let Some(child) = self
            .cost
            .as_ref()
            .and_then(|c| c.children.iter().find(|ch| ch.session == id))
        else {
            return String::new();
        };
        format!("  ·  {}", cost_short(&child.total))
    }

    /// `/cost` rows (51 §8.3): direct, descendants and total buckets, the
    /// estimate line, unresolved attempts, history coverage, remote-meter
    /// gaps, scope, then one row per immediate child.
    pub fn cost_rows(&self) -> Vec<String> {
        let Some(Menu::Cost { cost: c, .. }) = &self.menu else {
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

    /// `/cost`: a read-only list; esc closes.
    pub(super) fn cost_key(&mut self, code: KeyCode) -> MenuKey {
        let rows = self.cost_rows().len();
        let Some(Menu::Cost { cursor, .. }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(rows.saturating_sub(1)),
            KeyCode::Esc => self.menu = None,
            _ => {}
        }
        MenuKey::Consumed
    }
}
