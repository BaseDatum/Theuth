use super::*;

const NOW: u64 = 1_793_629_470_000; // 2026-11-02T14:24:30Z

fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

/// Anthropic API-key buckets for one response.
fn anthropic(date: &str, remaining: u64, reset: &str) -> Vec<(String, String)> {
    let r = remaining.to_string();
    h(&[
        ("date", date),
        ("anthropic-ratelimit-tokens-limit", "1000"),
        ("anthropic-ratelimit-tokens-remaining", &r),
        ("anthropic-ratelimit-tokens-reset", reset),
    ])
}

fn row<'a>(snap: &'a UsageLimits, id: &str) -> &'a ProviderLimits {
    snap.providers
        .iter()
        .find(|p| p.id == id)
        .unwrap_or_else(|| panic!("no row {id} in {:?}", snap.providers))
}

const D1: &str = "Mon, 02 Nov 2026 14:24:30 GMT";
const D2: &str = "Mon, 02 Nov 2026 14:24:31 GMT";
const RESET: &str = "2026-11-02T14:25:30Z";

#[test]
fn header_merge_newest_date_wins_tie_goes_to_lowest_remaining() {
    let mut st = State::default();
    assert!(st.observe(
        "anthropic",
        "api_key",
        "claude-opus-5-5",
        200,
        &anthropic(D2, 900, RESET),
        NOW
    ));
    // An older response arriving late does not overwrite the newer one.
    st.observe(
        "anthropic",
        "api_key",
        "claude-opus-5-5",
        200,
        &anthropic(D1, 100, RESET),
        NOW + 5,
    );
    let remaining = |st: &State| row(&st.render(NOW + 10), "anthropic:api_key").rates[0].remaining;
    assert_eq!(remaining(&st), 900, "older date loses");
    // Same date: the lower remaining wins, a higher one does not.
    st.observe(
        "anthropic",
        "api_key",
        "claude-opus-5-5",
        200,
        &anthropic(D2, 800, RESET),
        NOW + 6,
    );
    st.observe(
        "anthropic",
        "api_key",
        "claude-opus-5-5",
        200,
        &anthropic(D2, 850, RESET),
        NOW + 7,
    );
    assert_eq!(remaining(&st), 800, "tie → lowest remaining");
    // Another model group is its own bucket.
    st.observe(
        "anthropic",
        "api_key",
        "claude-haiku-5-5",
        200,
        &anthropic(D1, 990, RESET),
        NOW + 8,
    );
    let snap = st.render(NOW + 10);
    let scopes: Vec<_> = row(&snap, "anthropic:api_key")
        .rates
        .iter()
        .map(|r| (r.scope.clone().unwrap(), r.remaining))
        .collect();
    assert_eq!(
        scopes,
        [
            ("claude-haiku".to_owned(), 990),
            ("claude-opus".to_owned(), 800)
        ]
    );
}

#[test]
fn a_passed_reset_reads_full_until_re_observed() {
    let mut st = State::default();
    st.observe(
        "anthropic",
        "api_key",
        "claude-opus-5-5",
        200,
        &anthropic(D1, 100, RESET),
        NOW,
    );
    let reset_ms = 1_793_629_530_000;
    let before = st.render(reset_ms - 1);
    assert_eq!(row(&before, "anthropic:api_key").rates[0].remaining, 100);
    let after = st.render(reset_ms);
    assert_eq!(row(&after, "anthropic:api_key").rates[0].remaining, 1000);
}

#[test]
fn status_transitions_ok_warn_limited_ok_stale() {
    let mut st = State::default();
    let id = "anthropic:api_key";
    st.observe(
        "anthropic",
        "api_key",
        "claude-opus-5-5",
        200,
        &anthropic(D1, 500, RESET),
        NOW,
    );
    assert_eq!(row(&st.render(NOW + 1), id).status, LimitStatus::Ok);

    st.observe(
        "anthropic",
        "api_key",
        "claude-opus-5-5",
        200,
        &anthropic(D2, 150, RESET),
        NOW + 1_000,
    );
    let s = st.render(NOW + 1_001);
    assert_eq!(row(&s, id).status, LimitStatus::Warn, "≤ 15 % left");
    assert!(
        row(&s, id)
            .message
            .as_deref()
            .unwrap()
            .contains("15 % left")
    );

    let mut limited = anthropic("Mon, 02 Nov 2026 14:24:32 GMT", 0, RESET);
    limited.push(("retry-after".into(), "30".into()));
    st.observe(
        "anthropic",
        "api_key",
        "claude-opus-5-5",
        429,
        &limited,
        NOW + 2_000,
    );
    let s = st.render(NOW + 2_001);
    assert_eq!(row(&s, id).status, LimitStatus::Limited);
    assert!(
        row(&s, id)
            .message
            .as_deref()
            .unwrap()
            .starts_with("limited until")
    );
    assert_eq!(
        row(&st.render(NOW + 32_001), id).status,
        LimitStatus::Warn,
        "retry-after passed; the bucket is still empty until its reset"
    );
    assert_eq!(
        row(&st.render(1_793_629_530_000), id).status,
        LimitStatus::Ok,
        "the bucket's reset passed: reads full"
    );

    // A success with no limit headers lifts a fresh 429 — of its own model
    // scope only (review 160-LIM #3: a Haiku 200 never clears an Opus 429).
    st.observe(
        "anthropic",
        "api_key",
        "claude-opus-5-5",
        429,
        &limited,
        NOW + 40_000,
    );
    let opus = "claude-opus-5-5";
    assert!(!st.observe("anthropic", "api_key", opus, 500, &[], NOW + 40_001));
    assert!(!st.observe(
        "anthropic",
        "api_key",
        "claude-haiku-5-5",
        200,
        &[],
        NOW + 40_001
    ));
    st.observe(
        "anthropic",
        "api_key",
        "claude-haiku-5-5",
        200,
        &anthropic("Mon, 02 Nov 2026 14:25:10 GMT", 900, RESET),
        NOW + 40_001,
    );
    assert_eq!(
        row(&st.render(NOW + 40_002), id).status,
        LimitStatus::Limited,
        "another scope's 200 keeps the Opus 429"
    );
    assert!(st.observe("anthropic", "api_key", opus, 200, &[], NOW + 40_002));
    assert_ne!(
        row(&st.render(NOW + 40_003), id).status,
        LimitStatus::Limited
    );

    // No call for > 15 minutes since the last observation: stale.
    let s = st.render(NOW + 40_001 + 15 * MIN + 1);
    assert_eq!(row(&s, id).status, LimitStatus::Stale);
}

#[test]
fn window_thresholds_warn_at_75_limited_at_100() {
    let mut st = State::default();
    let codex = |pct: &str| {
        h(&[
            ("x-codex-primary-used-percent", pct),
            ("x-codex-primary-window-minutes", "300"),
            ("x-codex-primary-reset-at", "1793640000"),
        ])
    };
    st.observe("openai", "oauth", "gpt-6.1-sol", 200, &codex("74"), NOW);
    assert_eq!(row(&st.render(NOW), "openai:oauth").status, LimitStatus::Ok);
    st.observe("openai", "oauth", "gpt-6.1-sol", 200, &codex("75"), NOW);
    assert_eq!(
        row(&st.render(NOW), "openai:oauth").status,
        LimitStatus::Warn
    );
    st.observe("openai", "oauth", "gpt-6.1-sol", 200, &codex("100"), NOW);
    let s = st.render(NOW);
    assert_eq!(row(&s, "openai:oauth").status, LimitStatus::Limited);
    assert!(
        row(&s, "openai:oauth")
            .message
            .as_deref()
            .unwrap()
            .starts_with("5-hour limit reached")
    );
    // After the window resets it reads as unused.
    let s = st.render(1_793_640_000_000);
    assert_eq!(row(&s, "openai:oauth").windows[0].used_fraction, 0.0);
}

#[test]
fn polls_fill_rows_failures_keep_last_good() {
    let mut st = State::default();
    let data = PollData {
        windows: vec![UsageWindow {
            id: "primary".into(),
            label: "5-hour".into(),
            used_fraction: 0.3,
            resets_at_ms: Some(NOW + 3_600_000),
            window_secs: Some(18_000),
            history: Vec::new(),
        }],
        plan: Some("pro".into()),
        account: Some("acct-1234".into()),
        ..PollData::default()
    };
    st.apply_poll(Source::Codex, NOW, Ok(data));
    let s = st.render(NOW + 1);
    let r = row(&s, "openai:oauth");
    assert_eq!(
        (r.status, r.plan.as_deref()),
        (LimitStatus::Ok, Some("pro"))
    );
    assert_eq!(r.source, "usage endpoint");

    st.apply_poll(Source::Codex, NOW + 10, Err(PollError::Throttled));
    let s = st.render(NOW + 11);
    let r = row(&s, "openai:oauth");
    assert_eq!(r.status, LimitStatus::Stale, "429: keep last values, stale");
    assert_eq!(r.windows[0].used_fraction, 0.3, "last-good kept");

    st.apply_poll(
        Source::Codex,
        NOW + 20,
        Err(PollError::Hard("login expired — run /login openai".into())),
    );
    let s = st.render(NOW + 21);
    let r = row(&s, "openai:oauth");
    assert_eq!(r.status, LimitStatus::Error);
    assert_eq!(
        r.message.as_deref(),
        Some("login expired — run /login openai")
    );
    assert_eq!(r.windows.len(), 1, "last-good kept on error too");

    // Headers flowing again clear the failure; the row now has both sources.
    st.observe(
        "openai",
        "oauth",
        "gpt-6.1-sol",
        200,
        &h(&[("x-codex-primary-used-percent", "31")]),
        NOW + 30,
    );
    let s = st.render(NOW + 31);
    let r = row(&s, "openai:oauth");
    assert_eq!(
        (r.status, r.source.as_str()),
        (LimitStatus::Ok, "headers + usage endpoint")
    );

    // Codex polls hourly: stale only after two missed polls.
    let s = st.render(NOW + 30 + 119 * MIN);
    assert_eq!(row(&s, "openai:oauth").status, LimitStatus::Ok);
    let s = st.render(NOW + 30 + 121 * MIN);
    assert_eq!(row(&s, "openai:oauth").status, LimitStatus::Stale);
}

fn present(id: &str, provider: &str, a: &'static str, configured: bool) -> Present {
    Present {
        id: id.into(),
        provider: provider.into(),
        label: label(provider, a),
        auth: a,
        configured,
        fixed: None,
        headers: true,
        poll: None,
        account: None,
    }
}

#[test]
fn presence_unavailable_and_not_configured_rows() {
    let mut st = State::default();
    st.apply_presence(vec![
        present("anthropic:api_key", "anthropic", "api_key", true),
        Present {
            fixed: Some("Bedrock exposes no usage meter".into()),
            headers: false,
            ..present("amazon-bedrock", "amazon-bedrock", "aws", true)
        },
        present("openai:oauth", "openai", "oauth", false),
        present("openai:api_key", "openai", "api_key", false),
        present("deepseek:api_key", "deepseek", "api_key", false),
    ]);
    let ids = |st: &State| -> Vec<(String, LimitStatus)> {
        st.render(NOW)
            .providers
            .into_iter()
            .map(|p| (p.id, p.status))
            .collect()
    };
    assert_eq!(
        ids(&st),
        [
            ("anthropic:api_key".into(), LimitStatus::Stale),
            ("amazon-bedrock".into(), LimitStatus::Unavailable),
        ],
        "unconfigured providers without recent calls are omitted"
    );
    st.recent = ["openai".to_owned()].into();
    assert_eq!(
        ids(&st)[2],
        ("openai:api_key".into(), LimitStatus::NotConfigured),
        "one not-configured row per provider with recent calls"
    );
    assert_eq!(ids(&st).len(), 3);
    let s = st.render(NOW);
    assert_eq!(row(&s, "amazon-bedrock").source, "none");
    assert_eq!(
        row(&s, "anthropic:api_key").message.as_deref(),
        Some("no calls observed yet; limits arrive with the next call")
    );
}

#[test]
fn snapshot_round_trip_serves_stale_until_refreshed() {
    let dir = crate::TestDir::new("limits-snap");
    let path = dir.0.join("limits").join("snapshot.json");
    let mut st = State::default();
    st.observe(
        "anthropic",
        "api_key",
        "claude-opus-5-5",
        200,
        &anthropic(D1, 500, RESET),
        NOW,
    );
    st.apply_poll(
        Source::Codex,
        NOW,
        Ok(PollData {
            plan: Some("plus".into()),
            ..PollData::default()
        }),
    );
    st.budgets = vec![BudgetLimit {
        id: "routine:dreaming".into(),
        label: "dreaming".into(),
        period: "night".into(),
        cap_usd: 25.0,
        spent_usd: 3.5,
        resets_at_ms: None,
    }];
    let before = st.render(NOW + 1);
    save(&path, &before).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "owner-only");
    }
    let restored = State::restore(load(&path).unwrap());
    let after = restored.render(NOW + 1);
    assert_eq!(after.budgets, before.budgets);
    for (a, b) in after.providers.iter().zip(&before.providers) {
        assert_eq!(
            (&a.id, &a.rates, &a.windows, &a.plan),
            (&b.id, &b.rates, &b.windows, &b.plan)
        );
        assert_eq!((&a.source, a.observed_at_ms), (&b.source, b.observed_at_ms));
        assert_eq!(a.status, LimitStatus::Stale, "{} restored → stale", a.id);
    }
    assert_eq!(after.providers.len(), before.providers.len());

    // Fresh data lifts the restored flag.
    let mut restored = restored;
    restored.observe(
        "anthropic",
        "api_key",
        "claude-opus-5-5",
        200,
        &anthropic(D2, 400, RESET),
        NOW + 2,
    );
    assert_eq!(
        row(&restored.render(NOW + 3), "anthropic:api_key").status,
        LimitStatus::Ok
    );
    assert!(load(&dir.0.join("missing.json")).is_none());
}

// region: window history

const HOUR: u64 = 60 * MIN;

fn win(used: f64, resets_at_ms: u64) -> UsageWindow {
    UsageWindow {
        id: "primary".into(),
        label: "5-hour".into(),
        used_fraction: used,
        resets_at_ms: Some(resets_at_ms),
        window_secs: Some(18_000),
        history: Vec::new(),
    }
}

/// `(at_ms, used_fraction)` of the one window.
fn samples(ws: &[UsageWindow]) -> Vec<(u64, f64)> {
    ws[0]
        .history
        .iter()
        .map(|s| (s.at_ms, s.used_fraction))
        .collect()
}

#[test]
fn history_appends_on_change_and_every_five_minutes() {
    let reset = NOW + 4 * HOUR;
    let mut ws = Vec::new();
    merge_windows(&mut ws, vec![win(0.1, reset)], NOW);
    merge_windows(&mut ws, vec![win(0.2, reset)], NOW + 1_000);
    // Unchanged within 5 minutes: skipped.
    merge_windows(&mut ws, vec![win(0.2, reset)], NOW + 4 * MIN);
    assert_eq!(samples(&ws), [(NOW, 0.1), (NOW + 1_000, 0.2)]);
    // Unchanged, 5 minutes after the last sample: recorded.
    merge_windows(&mut ws, vec![win(0.2, reset)], NOW + 1_000 + 5 * MIN);
    assert_eq!(samples(&ws).len(), 3);
    assert_eq!(samples(&ws)[2], (NOW + 1_000 + 5 * MIN, 0.2));
    // Small jitter of the reset (a relative `reset_after`) is the same window.
    merge_windows(&mut ws, vec![win(0.25, reset + 2_000)], NOW + 6 * MIN);
    assert_eq!(samples(&ws).len(), 4);
}

#[test]
fn history_clears_on_a_new_window() {
    let reset = NOW + HOUR;
    let mut ws = Vec::new();
    merge_windows(&mut ws, vec![win(0.5, reset)], NOW);
    merge_windows(&mut ws, vec![win(0.6, reset)], NOW + MIN);
    // The reset moved a whole window forward: fresh history.
    merge_windows(&mut ws, vec![win(0.6, reset + 18_000_000)], NOW + 2 * MIN);
    assert_eq!(samples(&ws), [(NOW + 2 * MIN, 0.6)]);
    // A drop of more than 5 points (same reset): fresh history.
    merge_windows(&mut ws, vec![win(0.58, reset + 18_000_000)], NOW + 3 * MIN);
    assert_eq!(samples(&ws).len(), 2, "a small drop stays");
    merge_windows(&mut ws, vec![win(0.02, reset + 18_000_000)], NOW + 4 * MIN);
    assert_eq!(samples(&ws), [(NOW + 4 * MIN, 0.02)]);
}

#[test]
fn history_caps_at_300_keeping_first_and_newest() {
    let reset = NOW + 1_000 * HOUR;
    let mut ws = Vec::new();
    for i in 0..1_000u64 {
        merge_windows(&mut ws, vec![win(i as f64 * 1e-4, reset)], NOW + i);
        assert!(ws[0].history.len() <= 300);
    }
    let s = samples(&ws);
    assert_eq!(s[0].0, NOW, "first kept");
    assert_eq!(s.last().unwrap().0, NOW + 999, "newest kept");
    assert!(s.windows(2).all(|p| p[0].0 < p[1].0), "oldest first");
    assert!(s.len() > 200, "thinning halves only the older half");
}

#[test]
fn header_and_poll_share_one_history() {
    let mut st = State::default();
    let reset_s = (NOW + 2 * HOUR) / 1_000;
    let header = |pct: &str| {
        h(&[
            ("anthropic-ratelimit-unified-5h-utilization", pct),
            ("anthropic-ratelimit-unified-5h-reset", &reset_s.to_string()),
        ])
    };
    st.observe(
        "anthropic",
        "oauth",
        "claude-opus-5-5",
        200,
        &header("0.2"),
        NOW,
    );
    let poll = PollData {
        windows: vec![UsageWindow {
            id: "five_hour".into(),
            label: "5-hour".into(),
            used_fraction: 0.25,
            resets_at_ms: Some(reset_s * 1_000),
            window_secs: Some(18_000),
            history: Vec::new(),
        }],
        ..PollData::default()
    };
    st.apply_poll(Source::Claude, NOW + MIN, Ok(poll));
    st.observe(
        "anthropic",
        "oauth",
        "claude-opus-5-5",
        200,
        &header("0.3"),
        NOW + 2 * MIN,
    );
    let s = st.render(NOW + 3 * MIN);
    let r = row(&s, "anthropic:oauth");
    let w = r.windows.iter().find(|w| w.id == "five_hour").unwrap();
    assert_eq!(
        samples(std::slice::from_ref(w)),
        [(NOW, 0.2), (NOW + MIN, 0.25), (NOW + 2 * MIN, 0.3)]
    );
    // Past the reset the window reads unused, without the old readings.
    let s = st.render(reset_s * 1_000);
    let w = &row(&s, "anthropic:oauth").windows[0];
    assert!(w.history.is_empty());
}

#[test]
fn history_survives_the_snapshot_and_an_old_snapshot_loads() {
    let dir = crate::TestDir::new("limits-history");
    let path = dir.0.join("limits").join("snapshot.json");
    let mut st = State::default();
    st.apply_poll(
        Source::Codex,
        NOW,
        Ok(PollData {
            windows: vec![win(0.1, NOW + HOUR)],
            ..PollData::default()
        }),
    );
    st.apply_poll(
        Source::Codex,
        NOW + MIN,
        Ok(PollData {
            windows: vec![win(0.2, NOW + HOUR)],
            ..PollData::default()
        }),
    );
    save(&path, &st.render(NOW + 2 * MIN)).unwrap();
    let mut restored = State::restore(load(&path).unwrap());
    let r = row(&restored.render(NOW + 2 * MIN), "openai:oauth").clone();
    assert_eq!(samples(&r.windows), [(NOW, 0.1), (NOW + MIN, 0.2)]);
    // Fresh data after the restart continues the same history.
    restored.apply_poll(
        Source::Codex,
        NOW + 3 * MIN,
        Ok(PollData {
            windows: vec![win(0.3, NOW + HOUR)],
            ..PollData::default()
        }),
    );
    let r = row(&restored.render(NOW + 3 * MIN), "openai:oauth").clone();
    assert_eq!(samples(&r.windows).len(), 3);

    // A snapshot written before `history` existed.
    let mut json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for p in json["providers"].as_array_mut().unwrap() {
        for w in p["windows"].as_array_mut().unwrap() {
            w.as_object_mut().unwrap().remove("history");
        }
    }
    std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
    let old = load(&path).expect("old snapshot loads");
    let w = &row(&old, "openai:oauth").windows[0];
    assert_eq!((w.used_fraction, w.history.is_empty()), (0.2, true));
}

// endregion

#[test]
fn hub_api_observe_then_snapshot_and_kick_without_pollers() {
    let hub = LimitsHub::default();
    hub.kick(None);
    hub.kick(Some("openai"));
    hub.observe_headers(
        "anthropic",
        "api_key",
        "claude-opus-5-5",
        200,
        &anthropic(D1, 500, RESET),
        now_ms(),
    );
    let snap = hub.snapshot();
    assert_eq!(snap.providers.len(), 1);
    assert_eq!(snap.providers[0].label, "Anthropic API");
    assert!(
        LimitsHub::default().snapshot().providers.is_empty(),
        "tests own their hubs"
    );
}

#[test]
fn limits_config_switches() {
    let cfg: LimitsConfig = serde_json::from_value(serde_json::json!({
        "openai": {"poll": false},
        "anthropic": {"probe": true}
    }))
    .unwrap();
    assert!(!cfg.poll("openai"));
    assert!(cfg.poll("anthropic") && cfg.poll("deepseek"));
    assert!(cfg.probe("anthropic"));
    assert!(
        !LimitsConfig::default().probe("anthropic"),
        "no paid probe by default"
    );
}

#[test]
fn auth_modes() {
    use genai::adapter::AdapterKind;
    use genai::resolver::AuthData;
    let over = AuthData::RequestOverride {
        url: "https://chatgpt.com/backend-api/codex/responses".into(),
        headers: genai::Headers::from(Vec::<(String, String)>::new()),
    };
    assert_eq!(auth_mode(AdapterKind::OpenAIResp, &over, false), "oauth");
    assert_eq!(
        auth_mode(AdapterKind::Anthropic, &AuthData::None, false),
        "api_key"
    );
    assert_eq!(
        auth_mode(AdapterKind::BedrockSigv4, &AuthData::None, false),
        "aws"
    );
    assert_eq!(
        auth_mode(AdapterKind::Xai, &AuthData::Key("t".into()), true),
        "oauth"
    );
}

/// 160 brief §9: ONE `max_tokens: 1` Haiku call through `Backend` (≈ $1.5e-6),
/// then the global hub holds Anthropic rate rows. Needs `ANTHROPIC_API_KEY`.
#[tokio::test]
#[ignore = "live: one paid Anthropic call"]
async fn live_backend_call_fills_anthropic_rows() {
    use futures::StreamExt;
    let dir = crate::TestDir::new("limits-live");
    let client = genai::Client::builder().build().unwrap();
    let backend = crate::backend::Backend::Genai {
        client,
        auth: crate::supervisor::Auth::stub(&dir.0),
    };
    let resolved = models::Resolved {
        id: "anthropic/claude-haiku-5-5".into(),
        genai: "anthropic::claude-haiku-5-5".into(),
        provider: Some("anthropic".into()),
        model: None,
    };
    let req = genai::chat::ChatRequest::from_user("hi");
    let opts = genai::chat::ChatOptions::default()
        .with_max_tokens(16)
        .with_reasoning_effort(genai::chat::ReasoningEffort::Zero);
    let mut stream = backend
        .stream(&resolved, req, &opts, runtime::SessionId::new())
        .await
        .unwrap();
    while let Some(ev) = stream.next().await {
        ev.unwrap();
    }
    let snap = LimitsHub::global().snapshot();
    let r = row(&snap, "anthropic:api_key");
    let ids: Vec<_> = r.rates.iter().map(|r| r.id.as_str()).collect();
    assert!(
        ids.contains(&"requests") && ids.contains(&"output_tokens"),
        "{r:?}"
    );
    assert!(
        r.rates
            .iter()
            .all(|x| x.scope.as_deref() == Some("claude-haiku"))
    );
    eprintln!(
        "live: {} buckets, requests limit {}",
        r.rates.len(),
        r.rates[0].limit
    );
}
