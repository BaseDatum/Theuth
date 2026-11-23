//! 153 §16 rows 3–4 and the §5.4 test list.

use std::sync::{Arc, LazyLock};

use super::*;

static PACKAGED: LazyLock<Arc<packaged::Packaged>> =
    LazyLock::new(|| Arc::new(packaged::load().expect("packaged set loads")));

fn packaged() -> Arc<packaged::Packaged> {
    PACKAGED.clone()
}

fn with_packaged() -> Dictionary {
    Dictionary {
        packaged: packaged(),
        ..Dictionary::default()
    }
}

fn entry(canonical: &str, source: Source, spoken: &[&str], tags: &[&str]) -> Entry {
    Entry {
        id: EntryId(format!("{source:?}-{canonical}")),
        canonical: canonical.into(),
        kind: Kind::Term,
        spoken: spoken.iter().map(|s| s.to_string()).collect(),
        variants: Vec::new(),
        scope: Scope::Global,
        source,
        tags: tags.iter().map(|s| s.to_string()).collect(),
    }
}

fn variant(heard: &str) -> Variant {
    Variant {
        heard: heard.into(),
        metaphone: (String::new(), String::new()),
        seen: 1,
        last_ms: 0,
    }
}

fn canon(d: &Dictionary, heard: &str) -> Option<String> {
    d.lookup(heard).map(|m| m.canonical)
}

#[test]
fn packaged_loads_and_hashes() {
    let rec = packaged::record().unwrap();
    assert_eq!(rec.file, "dictionary.v1.json.gz");
    assert_eq!(rec.sha256, packaged::sha256(), "data file vs record sha256");
    let d = with_packaged();
    assert_eq!(d.counts().packaged as usize, rec.terms);
    // 204: server-side search over the packaged layer, normalised ("git hub" finds GitHub), capped.
    let s = d.search("git hub", Some(Source::Packaged), usize::MAX);
    assert!(s.entries.iter().any(|e| e.canonical == "GitHub"), "{:?}", s.entries);
    assert_eq!(s.total as usize, s.entries.len());
    assert!(d.search("git hub", Some(Source::User), 10).entries.is_empty());
    let s = d.search("e", None, 100);
    assert_eq!(s.entries.len(), 100);
    assert!(s.total > 100);
    assert_eq!(d.search(" - ", None, 100).total, 0);
    assert!(d.packaged_version().starts_with("v1 "));
    assert!(rec.terms >= 30_000 && rec.pairs >= 400, "{rec:?}");
    assert_eq!(canon(&d, "git hub").as_deref(), Some("GitHub"));
    assert_eq!(canon(&d, "cooper netties").as_deref(), Some("Kubernetes"));
    assert_eq!(canon(&d, "k eight s").as_deref(), Some("k8s"));
    assert!(
        d.pairs(Kind::Symbol)
            .any(|e| e.canonical == "," && e.spoken.iter().any(|s| s == "comma"))
    );
    assert!(
        d.pairs(Kind::Formatter)
            .any(|e| e.spoken.iter().any(|s| s == "camel case"))
    );
    assert!(d.pairs(Kind::Abbrev).count() >= 350);
    assert!(d.pairs(Kind::Path).any(|e| e.canonical == ".com"));
    // Pairs and number words are the shaper's, never a replacement.
    assert_eq!(canon(&d, "comma"), None);
    assert_eq!(canon(&d, "administrator"), None);
    assert_eq!(canon(&d, "seven"), None);
    assert_eq!(canon(&d, "com"), None);
}

#[test]
fn match_thresholds() {
    // "mestrel" vs "mistral": lev 2 of 7 + phonetic 0.1 → 0.814: in [ACCEPT_KNOWN, ACCEPT_NEW).
    let s = matcher::score("mestrel", "Mistral");
    assert!((ACCEPT_KNOWN..ACCEPT_NEW).contains(&s), "{s}");
    let mut d = Dictionary::default();
    d.put(entry("Mistral", Source::Learned, &[], &[]));
    assert_eq!(canon(&d, "mestrel"), None, "new: 0.81 < 0.85");
    assert_eq!(
        canon(&d, "mistrel").as_deref(),
        Some("Mistral"),
        "new: lev 1 → 0.957"
    );
    let mut d = Dictionary::default();
    d.put(entry("Mistral", Source::User, &[], &[]));
    assert_eq!(
        canon(&d, "mestrel").as_deref(),
        Some("Mistral"),
        "user entry: known threshold"
    );

    // A learned variant makes the heard form exact.
    let mut d = Dictionary::default();
    let mut e = entry("GitHub", Source::Learned, &[], &[]);
    e.variants.push(variant("get up"));
    let stored = d.put(e);
    assert!(
        !stored.variants[0].metaphone.0.is_empty(),
        "codes filled on put"
    );
    assert_eq!(canon(&d, "get up").as_deref(), Some("GitHub"));

    // Negative: vetoes, never creates.
    d.negative(Negative {
        heard_lc: "Get Up".into(),
        canonical: "GitHub".into(),
    });
    assert_eq!(canon(&d, "get up"), None);
    d.negative(Negative {
        heard_lc: "banana".into(),
        canonical: "GitHub".into(),
    });
    assert_eq!(canon(&d, "banana"), None);

    // Self-expansion guard.
    let mut d = Dictionary::default();
    d.put(entry("GitHub", Source::User, &["git"], &[]));
    assert_eq!(canon(&d, "git").as_deref(), Some("GitHub"));
    assert_eq!(canon(&d, "GitHub"), None);
    assert_eq!(canon(&d, "GitHub's"), None);
    assert_eq!(
        canon(&d, "github").as_deref(),
        Some("GitHub"),
        "case fix is not an expansion"
    );
}

#[test]
fn possessive_and_casing() {
    let mut d = with_packaged();
    assert_eq!(canon(&d, "kubernetes's").as_deref(), Some("Kubernetes's"));
    assert_eq!(
        canon(&d, "Npm"),
        None,
        "keep_case: the model's casing stands"
    );
    d.put(entry("foobar", Source::User, &[], &[]));
    assert_eq!(
        canon(&d, "Foo bar").as_deref(),
        Some("Foobar"),
        "sentence capital re-applied"
    );
    assert_eq!(canon(&d, "foo bar").as_deref(), Some("foobar"));
    d.put(entry(
        "ripgrep",
        Source::User,
        &["rip grep"],
        &["keep_case"],
    ));
    assert_eq!(canon(&d, "Rip grep").as_deref(), Some("ripgrep"));
    let m = d.lookup("foo bar").unwrap();
    assert_eq!(m.tokens, 2);
    assert_eq!(d.get(&m.entry).unwrap().canonical, "foobar");
}

#[test]
fn scope_filters() {
    let mut d = Dictionary::default();
    let mut e = entry("TuiOnly", Source::User, &["tooey only"], &[]);
    e.scope = Scope::Surface {
        surface: Surface::Tui,
    };
    d.put(e);
    let mut e = entry("AppOnly", Source::User, &["app only"], &[]);
    e.scope = Scope::App {
        app: "com.example".into(),
    };
    d.put(e);
    assert_eq!(canon(&d, "tooey only"), None);
    assert_eq!(
        d.lookup_scoped("tooey only", Some(Surface::Tui), None)
            .map(|m| m.canonical)
            .as_deref(),
        Some("TuiOnly")
    );
    assert_eq!(
        d.lookup_scoped("tooey only", Some(Surface::Composer), None),
        None
    );
    assert_eq!(
        d.lookup_scoped("app only", None, Some("com.example"))
            .map(|m| m.canonical)
            .as_deref(),
        Some("AppOnly")
    );
    assert_eq!(d.lookup_scoped("app only", None, Some("other")), None);
    assert_eq!(
        d.list(Some(&Scope::Surface {
            surface: Surface::Tui
        }))
        .len(),
        1
    );
    assert_eq!(d.list(None).len(), 2);
}

/// Index buckets vs a full scan over 1 000 terms (sampled from the packaged set, loaded as
/// learned entries so the fuzzy path applies): exact forms and lev-1 misspellings must pick
/// the same entry.
#[test]
fn bucket_prefilter_matches_brute_force() {
    let p = packaged();
    let pool: Vec<&Entry> = p
        .entries
        .iter()
        .filter(|e| replaces(e) && e.canonical.chars().count() >= 4)
        .collect();
    let mut d = Dictionary::default();
    for e in pool.iter().step_by(pool.len() / 1000).take(1000) {
        d.put(Entry {
            source: Source::Learned,
            tags: Vec::new(),
            ..(*e).clone()
        });
    }
    let entries = d.entries.clone();
    assert_eq!(entries.len(), 1000);
    let forms: Vec<Vec<(String, (String, String))>> = entries
        .iter()
        .map(|e| {
            std::iter::once(&e.canonical)
                .chain(&e.spoken)
                .map(|f| {
                    let n = matcher::norm(f);
                    let c = matcher::metaphone(&n);
                    (n, c)
                })
                .collect()
        })
        .collect();
    let brute = |heard: &str| -> Option<&Entry> {
        let key = matcher::norm(heard);
        let codes = matcher::metaphone(&key);
        let mut best: Option<(f32, usize)> = None;
        for (i, fs) in forms.iter().enumerate() {
            let phonetic = fs.iter().any(|(_, c)| {
                !c.0.is_empty()
                    && (c.0 == codes.0 || c.0 == codes.1 || c.1 == codes.0 || c.1 == codes.1)
            });
            // §5.4 step 1: an entry is a candidate only through an exact form or a shared code.
            if !phonetic && !fs.iter().any(|(n, _)| *n == key) {
                continue;
            }
            let s = fs
                .iter()
                .map(|(n, _)| {
                    if *n == key {
                        1.1 // an exact form beats a fuzzy 1.0
                    } else {
                        matcher::score_norm(&key, n, phonetic)
                    }
                })
                .fold(0.0, f32::max);
            if s >= ACCEPT_NEW && best.is_none_or(|(b, _)| s > b) {
                best = Some((s, i));
            }
        }
        best.map(|(_, i)| &entries[i])
            .filter(|e| e.canonical != heard)
    };
    let (mut exact_hits, mut fuzzy_hits) = (0, 0);
    for (i, e) in entries.iter().enumerate() {
        let upper = e.canonical.to_uppercase();
        let mut chars: Vec<char> = e.canonical.chars().collect();
        chars.remove(2);
        let typo: String = chars.into_iter().collect();
        for (label, heard, hits) in [
            ("exact", upper, &mut exact_hits),
            ("fuzzy", typo, &mut fuzzy_hits),
        ] {
            let got = d.lookup(&heard).map(|m| m.entry);
            let want = brute(&heard).map(|e| e.id.clone());
            assert_eq!(got, want, "{label} #{i} {heard:?}");
            *hits += got.is_some() as usize;
        }
    }
    println!("{exact_hits} exact, {fuzzy_hits} fuzzy hits");
    assert!(
        exact_hits >= 900 && fuzzy_hits >= 100,
        "{exact_hits} exact, {fuzzy_hits} fuzzy"
    );
}

#[test]
fn user_entries_round_trip() {
    let dir = std::env::temp_dir().join(format!("voice-dict-{}", uuid::Uuid::new_v4()));
    let path = store::path(&dir);
    let open = || {
        let (entries, negatives) = store::read(&path).unwrap();
        let mut d = Dictionary {
            packaged: packaged(),
            entries,
            negatives,
            index: matcher::Index::default(),
            path: Some(path.clone()),
        };
        d.index = matcher::Index::build(&d.entries);
        d
    };
    let mut d = open();
    assert_eq!(d.counts().user, 0);
    let a = d.put(entry("Theuth", Source::User, &["tooth"], &[]));
    let mut b = entry("Parakeet", Source::Learned, &[], &[]);
    b.id = EntryId(String::new());
    b.variants.push(variant("pair of keet"));
    let b = d.put(b);
    assert!(!b.id.0.is_empty(), "id assigned");
    d.negative(Negative {
        heard_lc: "tooth".into(),
        canonical: "Parakeet".into(),
    });
    assert!(path.exists());
    assert!(
        std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".tmp")),
        "no temp left behind"
    );

    let d2 = open();
    assert_eq!(d2.list(None), d.list(None));
    assert_eq!(d2.negatives, d.negatives);
    assert_eq!(d2.counts().user, 1);
    assert_eq!(d2.counts().learned, 1);
    assert_eq!(canon(&d2, "tooth").as_deref(), Some("Theuth"));
    assert_eq!(canon(&d2, "pair of keet").as_deref(), Some("Parakeet"));

    // Upsert by canonical + scope keeps one row; delete persists.
    let mut d = open();
    d.put(entry("Theuth", Source::User, &["tooth", "thoth"], &[]));
    assert_eq!(d.list(None).len(), 2);
    assert!(d.delete(&a.id));
    assert!(!d.delete(&a.id));
    assert_eq!(open().list(None).len(), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// 1 000 lookups over the packaged set in debug — the §5.4 step-7 bound.
#[test]
fn lookup_timing() {
    let d = with_packaged();
    let heard: Vec<String> = d
        .packaged
        .entries
        .iter()
        .step_by(40)
        .take(1000)
        .map(|e| e.canonical.to_uppercase())
        .collect();
    let t = std::time::Instant::now();
    let hits = heard.iter().filter(|h| d.lookup(h).is_some()).count();
    let dt = t.elapsed();
    println!("1000 lookups: {dt:?}");
    assert!(hits > 0);
    assert!(dt.as_millis() < 50, "1000 lookups took {dt:?}");
}

/// Prose must survive the 40k-term packaged set untouched (A2 as-built: with fuzzy packaged
/// matching and source aliases on, 108 of these 300 forms were rewritten — "and" → wand,
/// "plan" → Plane, "man" → Roff, "play" → Play).
#[test]
fn prose_is_left_alone() {
    let d = with_packaged();
    let words = "the quick brown fox jumps over the lazy dog please send this message to my manager about the meeting tomorrow morning we should talk about the budget and the plan for next quarter I think the design is good but the colors are wrong can you fix the bug in the login page and deploy it tonight thanks a lot see you later hello world this is a test of the dictation system it works pretty well so far maybe we need more words like apple banana cherry house mouse table chair window door garden street city country river mountain ocean cloud rain snow sun moon star light dark fast slow big small new old good bad happy sad love hate work play read write speak listen watch look find lose open close start stop begin end first last high low left right up down in out yes no maybe always never often sometimes today yesterday tomorrow week month year hour minute second number letter word sentence paragraph page book story music song film movie game sport team player coach school teacher student lesson question answer problem solution idea thought mind body hand foot head heart face eye ear nose mouth hair skin blood bone power energy force speed time space place thing person people man woman child boy girl friend family mother father sister brother son daughter";
    let mut rewritten = Vec::new();
    for w in words.split_whitespace() {
        let mut c = w.chars();
        let capitalised: String = c.next().unwrap().to_uppercase().chain(c).collect();
        for h in [w.to_string(), capitalised] {
            if let Some(m) = d.lookup(&h) {
                rewritten.push(format!("{h} -> {} ({:.2})", m.canonical, m.score));
            }
        }
    }
    assert!(rewritten.is_empty(), "{rewritten:#?}");
}
