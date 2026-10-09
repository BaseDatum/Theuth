//! 153 §4.3 examples table and the §4.7 list.

use crate::dictionary::{EntryId, Match};

use super::*;

/// A tiny dictionary: exact lowercase window → canonical (A2's matcher is the real one).
struct Fake(Vec<(&'static str, &'static str)>);

impl Lookup for Fake {
    fn lookup(&self, heard: &str) -> Option<Match> {
        let h = heard.to_lowercase();
        self.0.iter().find(|(k, _)| *k == h).map(|(k, v)| Match {
            entry: EntryId(format!("fake:{k}")),
            canonical: (*v).to_owned(),
            score: 1.0,
            tokens: k.split(' ').count() as u8,
        })
    }
}

fn fake() -> Fake {
    Fake(vec![
        ("parakeet", "Parakeet"),
        ("tee dee tee", "TDT"),
        ("git hub", "GitHub"),
        ("github", "GitHub"),
    ])
}

struct Harness {
    cfg: ShapeConfig,
    phrases: EndPhrases,
    dict: Fake,
    terminal: bool,
    commands: Vec<String>,
    mentions: Vec<String>,
}

impl Default for Harness {
    fn default() -> Self {
        Self {
            cfg: Preset::Full.into(),
            phrases: EndPhrases::default(),
            dict: fake(),
            terminal: false,
            commands: vec!["commit".into(), "compact".into()],
            mentions: vec!["theuth".into()],
        }
    }
}

impl Harness {
    fn ctx(&self) -> Ctx<'_> {
        Ctx {
            cfg: self.cfg,
            phrases: &self.phrases,
            dict: &self.dict,
            terminal: self.terminal,
            commands: &self.commands,
            mentions: &self.mentions,
            tlds: &[],
            exts: &[],
        }
    }

    fn shape(&self, raw: &str, is_final: bool) -> Shaped {
        self.ctx()
            .run(&mut EndParser::new(&self.phrases), "", raw, is_final)
    }

    fn text(&self, raw: &str) -> String {
        self.shape(raw, true).text
    }
}

const SEND: EndCommand = EndCommand::Send { near_miss: false };

struct Row {
    n: u32,
    raw: &'static str,
    want: &'static str,
    is_final: bool,
    /// A partial fed first (row 27 arms on it).
    prime: Option<&'static str>,
    terminal: bool,
    command: Option<EndCommand>,
}

const fn row(n: u32, raw: &'static str, want: &'static str) -> Row {
    Row {
        n,
        raw,
        want,
        is_final: true,
        prime: None,
        terminal: false,
        command: None,
    }
}

const ROWS: &[Row] = &[
    row(
        1,
        "Hi Dana comma the deploy is done period",
        "Hi Dana, the deploy is done.",
    ),
    row(2, "open paren see below close paren", "(see below)"),
    row(3, "the plus side of this", "the plus side of this"),
    row(4, "two plus two equals four", "2 + 2 = 4"),
    row(5, "send it to theuth dot dev", "send it to theuth.dev"),
    row(6, "connect the dots here", "connect the dots here"),
    row(7, "deploy dot S H", "deploy.sh"),
    row(8, "version two point five", "version 2.5"),
    row(9, "slash commit", "/commit"),
    row(10, "I'll slash the budget", "I'll slash the budget"),
    row(11, "tilde slash develop slash theuth", "~/develop/theuth"),
    row(
        12,
        "dash dash dry run flag on deploy dot S H",
        "--dry-run flag on deploy.sh",
    ),
    row(13, "dash V", "-V"),
    row(14, "a well dash known bug", "a well-known bug"),
    row(15, "snake case max open secs", "max_open_secs"),
    row(16, "camel case on text delta", "onTextDelta"),
    row(17, "pascal case answer card", "AnswerCard"),
    row(
        18,
        "underscore underscore init underscore underscore",
        "__init__",
    ),
    row(19, "at theuth can you check", "@theuth can you check"),
    row(20, "at theuth's desk", "at theuth's desk"),
    row(21, "new line thanks comma Sky", "\nThanks, Sky"),
    row(
        22,
        "first paragraph new paragraph second",
        "first paragraph\n\nsecond",
    ),
    row(23, "twenty three files", "23 files"),
    row(24, "the third option", "the third option"),
    row(25, "literal send it", "send it"),
    Row {
        command: Some(SEND),
        ..row(26, "ship it tonight send it", "ship it tonight")
    },
    Row {
        prime: Some("ship it tonight send it"),
        command: Some(EndCommand::Send { near_miss: true }),
        ..row(27, "ship it tonight sent it", "ship it tonight")
    },
    row(28, "hello, comma world", "hello, world"),
    row(29, "parakeet tee dee tee model", "Parakeet TDT model"),
    row(30, "git hub actions", "GitHub actions"),
    Row {
        terminal: true,
        ..row(31, "slash compact", "/compact")
    },
    Row {
        terminal: true,
        ..row(32, "pipe grep voice", "| grep voice")
    },
    row(33, "backtick cargo test backtick", "`cargo test`"),
    row(34, "hash one five three", "#153"),
    Row {
        is_final: false,
        ..row(35, "so yeah period", "so yeah.")
    },
];

#[test]
fn examples_table_rows_1_to_35() {
    let mut failed = Vec::new();
    for r in ROWS {
        let h = Harness {
            terminal: r.terminal,
            ..Harness::default()
        };
        let mut parser = EndParser::new(&h.phrases);
        if let Some(p) = r.prime {
            h.ctx().run(&mut parser, "", p, false);
        }
        let got = h.ctx().run(&mut parser, "", r.raw, r.is_final);
        if got.text != r.want || got.command != r.command {
            failed.push(format!(
                "row {}: {:?} → text {:?} (want {:?}), command {:?} (want {:?})",
                r.n, r.raw, got.text, r.want, got.command, r.command
            ));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    // Row 30's guard: a second pass over the output leaves it.
    let h = Harness::default();
    assert_eq!(h.text("GitHub actions"), "GitHub actions");
}

#[test]
fn spacing_classes_never_double_space() {
    let h = Harness::default();
    for r in ROWS {
        let t = h.text(r.raw);
        assert!(!t.contains("  "), "{:?} → {t:?}", r.raw);
        assert!(
            !t.contains(" ,") && !t.contains(" ."),
            "{:?} → {t:?}",
            r.raw
        );
        assert!(
            !t.ends_with(' ') && !t.starts_with(' '),
            "{:?} → {t:?}",
            r.raw
        );
    }
    assert_eq!(h.text("comma period comma"), ",.,");
    assert_eq!(
        h.text("open paren open paren x close paren close paren"),
        "((x))"
    );
}

#[test]
fn dot_gate_tld_list_and_extensions() {
    let h = Harness::default();
    assert_eq!(h.text("theuth dot dev"), "theuth.dev");
    assert_eq!(h.text("main dot rs"), "main.rs");
    assert_eq!(h.text("example dot com slash docs"), "example.com/docs");
    assert_eq!(h.text("the dot here"), "the dot here");
    assert_eq!(h.text("dot dev"), "dot dev");
    // A partial leaves the gate for the final (the TLD may still be growing).
    assert_eq!(h.shape("theuth dot dev", false).text, "theuth dot dev");
    assert_eq!(
        h.shape("theuth dot dev please", false).text,
        "theuth.dev please"
    );
}

#[test]
fn slash_gate_needs_path_context() {
    let h = Harness::default();
    assert_eq!(h.text("slash the budget"), "slash the budget");
    assert_eq!(h.text("dot slash deploy dot S H"), "./deploy.sh");
    assert_eq!(h.text("run slash commit"), "run /commit");
    assert_eq!(h.text("please slash commit"), "please slash commit");
    assert_eq!(h.text("forward slash etc"), "/etc");
    assert_eq!(h.text("back slash n"), "\\n");
}

#[test]
fn flag_join_stops_at_function_word() {
    let h = Harness::default();
    assert_eq!(
        h.text("dash dash no cache and then build"),
        "--no-cache and then build"
    );
    assert_eq!(h.text("dash dash verbose"), "--verbose");
    assert_eq!(h.text("dash dash foo bar baz qux"), "--foo-bar-baz qux");
    assert_eq!(h.text("dash dash dry run comma then"), "--dry-run, then");
}

#[test]
fn identifier_formatters_consume_at_most_four_words() {
    let h = Harness::default();
    assert_eq!(h.text("snake case a b c d e"), "a_b_c_d e");
    assert_eq!(h.text("kebab case voice any where"), "voice-any-where");
    assert_eq!(h.text("constant case max open secs"), "MAX_OPEN_SECS");
    assert_eq!(h.text("all caps read me"), "READ ME");
    assert_eq!(h.text("dotted voice shape mod"), "voice.shape.mod");
    assert_eq!(h.text("camel case one comma two"), "one, 2");
    assert_eq!(h.text("snake case"), "snake case");
}

#[test]
fn numbers_cap_six_digits() {
    let h = Harness::default();
    assert_eq!(
        h.text("nine hundred ninety nine thousand nine hundred ninety nine"),
        "999999"
    );
    assert_eq!(h.text("one million"), "one million");
    assert_eq!(h.text("two thousand twenty five"), "2025");
    assert_eq!(h.text("eighteen hundred"), "eighteen hundred");
    assert_eq!(h.text("one of them"), "one of them");
    assert_eq!(h.text("one point five percent"), "1.5%");
    assert_eq!(h.text("number third"), "number 3rd");
    assert_eq!(h.text("twenty-three"), "23");
}

#[test]
fn model_punctuation_cleanup_only_adjacent() {
    let h = Harness::default();
    assert_eq!(h.text("hello, comma world."), "hello, world.");
    assert_eq!(h.text("hello, world comma"), "hello, world,");
    assert_eq!(h.text("new line."), "\n");
    assert_eq!(h.text("done. new paragraph next"), "done.\n\nnext");
    assert_eq!(h.text("done. new line next"), "done.\nNext");
}

#[test]
fn dictionary_self_expansion_guard() {
    let h = Harness::default();
    let s = h.shape("git hub actions", true);
    assert_eq!(s.text, "GitHub actions");
    assert_eq!(
        s.replaced,
        vec![Replaced {
            start: 0,
            end: 6,
            heard: "git hub".into(),
            rule: RuleId("fake:git hub".into())
        }]
    );
    let again = h.shape(&s.text, true);
    assert_eq!(again.text, "GitHub actions");
    assert!(again.replaced.is_empty());
    // Path pieces are never rewritten.
    assert_eq!(h.text("parakeet dot rs"), "parakeet.rs");
}

#[test]
fn end_command_arms_on_partial_and_sticks() {
    let h = Harness::default();
    let mut p = EndParser::new(&h.phrases);
    let a = h.ctx().run(&mut p, "", "ship it send it", false);
    assert_eq!((a.text.as_str(), a.command), ("ship it", Some(SEND)));
    let b = h.ctx().run(&mut p, "", "ship it sent", false);
    assert_eq!((b.text.as_str(), b.command), ("ship it sent", Some(SEND)));
    let c = h.ctx().run(&mut p, "", "ship it sent it.", true);
    assert_eq!(
        (c.text.as_str(), c.command),
        ("ship it", Some(EndCommand::Send { near_miss: true }))
    );
    // Off switch: nothing armed, phrase stays.
    let plain = Harness {
        cfg: Preset::Plain.into(),
        ..Harness::default()
    };
    let d = plain.shape("ship it send it", false);
    assert_eq!((d.text.as_str(), d.command), ("ship it send it", None));
}

#[test]
fn near_miss_tolerance_len_over_5_min_5_chars() {
    let h = Harness {
        phrases: EndPhrases {
            send: vec!["send".into()],
            ..EndPhrases::default()
        },
        ..Harness::default()
    };
    let mut p = EndParser::new(&h.phrases);
    assert_eq!(
        h.ctx().run(&mut p, "", "ship it send", false).command,
        Some(SEND)
    );
    let f = h.ctx().run(&mut p, "", "ship it end", true);
    assert_eq!((f.text.as_str(), f.command), ("ship it end", None));
    // Unarmed: a near miss at the final is just words.
    assert_eq!(
        Harness::default().shape("ship it sent it", true).command,
        None
    );
}

#[test]
fn literal_escapes_end_phrase_and_symbol() {
    let h = Harness::default();
    assert_eq!(h.shape("literal send it", true).command, None);
    assert_eq!(h.text("say literal comma here"), "say comma here");
    assert_eq!(h.text("literal new line"), "new line");
    assert_eq!(h.text("literal scratch that"), "scratch that");
    assert_eq!(h.text("type literal slash commit"), "type slash commit");
}

#[test]
fn terminal_spacing_only_on_terminal_surfaces() {
    let tui = Harness {
        terminal: true,
        ..Harness::default()
    };
    let composer = Harness::default();
    for raw in ["slash compact", "at theuth"] {
        assert!(!tui.text(raw).ends_with(char::is_whitespace));
        assert_eq!(tui.text(raw), composer.text(raw));
    }
    assert_eq!(tui.text("slash compact new line"), "/compact\n");
    let off = Harness {
        terminal: true,
        cfg: ShapeConfig {
            terminal_spacing: false,
            ..Preset::Full.into()
        },
        ..Harness::default()
    };
    assert_eq!(off.text("slash compact"), "/compact");
}

#[test]
fn partial_then_final_is_idempotent_on_plain_prose() {
    let h = Harness::default();
    for raw in [
        "so yeah period",
        "Hi Dana comma the deploy is done period",
        "the plus side of this",
    ] {
        let partial = h.shape(raw, false);
        let fin = h.shape(raw, true);
        assert_eq!(partial, fin, "{raw:?}");
        assert_eq!(h.shape(&fin.text, true).text, fin.text, "{raw:?}");
    }
}

#[test]
fn shape_is_total_over_fuzzed_utf8() {
    const POOL: &[&str] = &[
        "dot",
        "slash",
        "dash",
        "comma",
        "send",
        "it",
        "literal",
        "new",
        "line",
        "point",
        "two",
        "one",
        "hundred",
        "camel",
        "case",
        "at",
        "theuth",
        "tilde",
        "underscore",
        "quote",
        "backtick",
        "percent",
        "plus",
        "S",
        "H",
        "git",
        "hub",
        "number",
        "third",
        ",",
        ".",
        "?",
        "(",
        ")",
        "\"",
        "é",
        "ß",
        "日本",
        "🦀",
        "\u{301}",
        "\u{200d}",
        "ﬁ",
        "İ",
        "\t",
        "\n",
        "'",
        "a's",
        "--",
        "~",
        "/",
        "..",
        "...",
        "",
        " ",
    ];
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let h = Harness::default();
    let mut p = EndParser::new(&h.phrases);
    for _ in 0..10_000 {
        let n = (next() % 12) as usize;
        let mut raw = String::new();
        for _ in 0..n {
            let r = next();
            if r % 7 == 0 {
                raw.push(char::from_u32((r >> 8) as u32 % 0x11_0000).unwrap_or('x'));
            } else {
                raw.push_str(POOL[(r >> 8) as usize % POOL.len()]);
            }
            raw.push(if r % 5 == 0 { '\u{a0}' } else { ' ' });
        }
        let is_final = next() % 3 == 0;
        let s = h.ctx().run(&mut p, "x\n", &raw, is_final);
        for r in &s.replaced {
            assert!(s.text.is_char_boundary(r.start) && s.text.is_char_boundary(r.end));
        }
        if is_final {
            p.reset();
        }
    }
}

#[test]
fn presets_full_plain_off_resolve() {
    let full: ShapeConfig = Preset::Full.into();
    let plain: ShapeConfig = Preset::Plain.into();
    let off: ShapeConfig = Preset::Off.into();
    assert!(
        full.punctuation && full.literals && full.numbers && full.dictionary && full.end_commands
    );
    assert!(plain.punctuation && plain.literals && plain.numbers);
    assert!(!plain.dictionary && !plain.end_commands && !plain.terminal_spacing);
    assert_eq!(
        off,
        ShapeConfig {
            punctuation: false,
            literals: false,
            numbers: false,
            dictionary: false,
            end_commands: false,
            terminal_spacing: false
        }
    );
    let raw = "git hub comma twenty three send it";
    let h = |cfg| Harness {
        cfg,
        ..Harness::default()
    };
    assert_eq!(h(off).shape(raw, true).text, raw);
    assert_eq!(h(plain).shape(raw, true).text, "git hub, 23 send it");
    let f = h(full).shape(raw, true);
    assert_eq!((f.text.as_str(), f.command), ("GitHub, 23", Some(SEND)));
    assert_eq!(ShapePresets::default().get(Surface::Tui), Preset::Full);
}

#[test]
fn scratch_that_drops_previous_final_only() {
    let h = Harness::default();
    let mut p = EndParser::new(&h.phrases);
    let a = h.ctx().run(&mut p, "", "scratch that", false);
    assert_eq!(
        (a.text.as_str(), a.command),
        ("", Some(EndCommand::ScratchThat))
    );
    let b = h.ctx().run(&mut p, "", "scratch that.", true);
    assert_eq!(
        (b.text.as_str(), b.command),
        ("", Some(EndCommand::ScratchThat))
    );
    p.reset();
    // Mid-utterance it is prose; only the tail is a command.
    let c = h
        .ctx()
        .run(&mut p, "", "scratch that idea and start over", true);
    assert_eq!(
        (c.text.as_str(), c.command),
        ("scratch that idea and start over", None)
    );
}

#[test]
fn shaper_builders_reach_the_stages() {
    let dict = crate::dictionary::Dictionary::default();
    let phrases = EndPhrases::default();
    let bare = Shaper::new(Preset::Full.into(), &phrases, &dict, None);
    assert_eq!(
        bare.shape("", "slash commit at theuth", true).text,
        "slash commit at theuth"
    );
    let s = Shaper::new(Preset::Full.into(), &phrases, &dict, None)
        .with_surface(Surface::Tui)
        .with_commands(["commit".to_owned()])
        .with_mentions(["theuth".to_owned()]);
    let out = s.shape("", "slash commit at theuth send it.", true);
    assert_eq!(
        (out.text.as_str(), out.command),
        ("/commit @theuth", Some(SEND))
    );
    // The stateless form sees only exact phrases; the actor's parser carries the arming.
    assert_eq!(s.shape("", "ship it sent it", true).command, None);
    let mut p = EndParser::new(&phrases);
    s.shape_with(&mut p, "", "ship it send it", false);
    assert_eq!(
        s.shape_with(&mut p, "", "ship it sent it", true).command,
        Some(EndCommand::Send { near_miss: true })
    );
}
