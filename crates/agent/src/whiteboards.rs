//! 218 §3.2, §5: whiteboards bound to sessions — the `<whiteboard>` outline as the agent's baseline,
//! the `<whiteboard_changes>` note on later turns, the send context the composer hands over, and the
//! turn boundaries the document crate keeps for *Undo this turn*.
//!
//! The document crate renders `<whiteboard>`; this module never compares text. It parses the
//! baseline (the last full block in the transcript, with every later change note applied) and the
//! fresh outline with the same parser and diffs the parsed [`Facts`], so a field the parser ignores
//! can never produce a false change. Each bound session has its own transcript, hence its own
//! baseline: a mark sent to one session still reaches every other bound session once.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use runtime::SessionId;

use crate::rpc_whiteboards::WbSendContext;
use crate::store::Store;

/// At most this many `+`/`-`/`~` lines in one `<whiteboard_changes>` note (§3.2).
pub const MAX_CHANGES: usize = 40;
/// A session bound to more whiteboards gets the full block for this many, a brief line for the rest.
pub const MAX_FULL: usize = 3;
/// A pending send context is dropped after this long (§4.2).
pub const CONTEXT_TTL: Duration = Duration::from_secs(120);

const SEP: &str = " · ";

// ---------------------------------------------------------------------------------------------
// Facts: what the outline says, parsed.
// ---------------------------------------------------------------------------------------------

/// Where a card sits in the reading order: a frame (`fN`) or the loose cards, and a row (from 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// Inside frame `fN`.
    Frame(u32),
    /// Outside every frame.
    Loose,
}

impl Place {
    fn render(self, row: u32) -> String {
        match self {
            Place::Frame(n) => format!("f{n} row {row}"),
            Place::Loose => format!("loose row {row}"),
        }
    }

    /// `fN row R` | `loose row R`.
    fn parse(s: &str) -> Option<(Place, u32)> {
        let (place, row) = s.split_once(" row ")?;
        let row = row.parse().ok()?;
        let place = if place == "loose" {
            Place::Loose
        } else {
            Place::Frame(num(place, 'f')?)
        };
        Some((place, row))
    }
}

/// A card's listed state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardState {
    /// `live`.
    Live,
    /// `dimmed` (with its tag).
    Dimmed,
    /// `placeholder`: reserved, not filled yet.
    Placeholder,
}

impl CardState {
    fn as_str(self) -> &'static str {
        match self {
            CardState::Live => "live",
            CardState::Dimmed => "dimmed",
            CardState::Placeholder => "placeholder",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "live" => CardState::Live,
            "dimmed" => CardState::Dimmed,
            "placeholder" => CardState::Placeholder,
            _ => return None,
        })
    }
}

/// One card line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    /// `page`, `sticky`, … (kept as written).
    pub kind: String,
    /// The title (unquoted).
    pub title: String,
    /// `vK/K`: the version count; `None` when the line has none (unversioned kinds, placeholders).
    pub versions: Option<u32>,
    /// Listed state.
    pub state: CardState,
    /// The dimmed reason.
    pub tag: Option<String>,
    /// Everything after state and tag, opaque (`" · 3 lines · #7419"`), empty when none.
    pub extras: String,
    /// Its row, from the `row` lines.
    pub at: Option<(Place, u32)>,
}

impl Card {
    /// The outline line after the alias: `KIND QSTR[ vK/K] · STATE[ QSTR][ · EXTRA…]`.
    fn line(&self, n: u32) -> String {
        let mut s = format!("c{n} {} {}", self.kind, quote(&self.title, 80));
        if let Some(k) = self.versions {
            let _ = write!(s, " v{k}/{k}");
        }
        s.push_str(SEP);
        s.push_str(self.state.as_str());
        if let Some(t) = &self.tag {
            s.push(' ');
            s.push_str(&quote(t, 120));
        }
        s.push_str(&self.extras);
        s
    }

    /// Parses what follows `cN `.
    fn parse(rest: &str) -> Option<Card> {
        let (kind, rest) = rest.split_once(' ')?;
        let (title, mut rest) = unquote(rest)?;
        let mut versions = None;
        if let Some(r) = rest.strip_prefix(" v") {
            let end = r.find(' ').unwrap_or(r.len());
            let (_, k) = r[..end].split_once('/')?;
            versions = Some(k.parse().ok()?);
            rest = &r[end..];
        }
        let rest = rest.strip_prefix(SEP)?;
        let end = rest.find(' ').unwrap_or(rest.len());
        let state = CardState::parse(&rest[..end])?;
        let mut rest = &rest[end..];
        let mut tag = None;
        if let Some(r) = rest.strip_prefix(' ')
            && r.starts_with('"')
        {
            let (t, r) = unquote(r)?;
            tag = Some(t);
            rest = r;
        }
        if !rest.is_empty() && !rest.starts_with(SEP) {
            return None;
        }
        Some(Card {
            kind: kind.to_owned(),
            title,
            versions,
            state,
            tag,
            extras: rest.to_owned(),
            at: None,
        })
    }

    fn versioned(&self) -> bool {
        !matches!(self.kind.as_str(), "sticky" | "note" | "diagram")
    }
}

/// One frame line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// The title (unquoted).
    pub title: String,
    /// `phone 390×844` when the frame has a device preset.
    pub preset: Option<String>,
}

impl Frame {
    fn line(&self, n: u32) -> String {
        let mut s = format!("frame f{n} {}", quote(&self.title, 80));
        if let Some(p) = &self.preset {
            s.push_str(SEP);
            s.push_str(p);
        }
        s
    }

    /// Parses what follows `frame fN `.
    fn parse(rest: &str) -> Option<Frame> {
        let (title, rest) = unquote(rest)?;
        let preset = match rest {
            "" => None,
            r => Some(r.strip_prefix(SEP)?.to_owned()),
        };
        Some(Frame { title, preset })
    }
}

/// One link line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// From card `cA`.
    pub from: u32,
    /// `→`, `↔` or `—`.
    pub arrow: String,
    /// To card `cB`.
    pub to: u32,
    /// Its label.
    pub label: Option<String>,
}

impl Link {
    fn ends(&self) -> String {
        format!("c{} {} c{}", self.from, self.arrow, self.to)
    }

    fn line(&self, n: u32) -> String {
        let mut s = format!("l{n} {}", self.ends());
        if let Some(l) = &self.label {
            s.push(' ');
            s.push_str(&quote(l, 80));
        }
        s
    }

    /// `cA ARROW cB`, then the rest.
    fn parse_ends(s: &str) -> Option<(u32, String, u32, &str)> {
        let mut it = s.splitn(3, ' ');
        let from = num(it.next()?, 'c')?;
        let arrow = it.next()?;
        if !matches!(arrow, "→" | "↔" | "—") {
            return None;
        }
        let rest = it.next()?;
        let end = rest.find(' ').unwrap_or(rest.len());
        let to = num(&rest[..end], 'c')?;
        Some((from, arrow.to_owned(), to, &rest[end..]))
    }

    /// Parses what follows `lN `.
    fn parse(rest: &str) -> Option<Link> {
        let (from, arrow, to, rest) = Self::parse_ends(rest)?;
        let label = match rest {
            "" => None,
            r => {
                let (l, tail) = unquote(r.strip_prefix(' ')?)?;
                if !tail.is_empty() {
                    return None;
                }
                Some(l)
            }
        };
        Some(Link {
            from,
            arrow,
            to,
            label,
        })
    }
}

/// A listed mark's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkState {
    /// Sent, not dealt with yet.
    Sent,
    /// Pinned by the owner.
    Pinned,
}

impl MarkState {
    fn as_str(self) -> &'static str {
        match self {
            MarkState::Sent => "sent",
            MarkState::Pinned => "pinned",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "sent" => Some(MarkState::Sent),
            "pinned" => Some(MarkState::Pinned),
            _ => None,
        }
    }
}

/// One mark line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mark {
    /// `c3 v2` or `canvas`.
    pub on: String,
    /// Listed state.
    pub state: MarkState,
    /// Caption and handwriting, opaque (`" · \"page says: …\" · handwriting: \"…\""`).
    pub rest: String,
}

impl Mark {
    fn line(&self, n: u32) -> String {
        format!("m{n} on {}{SEP}{}{}", self.on, self.state.as_str(), self.rest)
    }

    /// Parses what follows `mN on `.
    fn parse(rest: &str) -> Option<Mark> {
        let (on, rest) = rest.split_once(SEP)?;
        let end = rest.find(SEP).unwrap_or(rest.len());
        let state = MarkState::parse(&rest[..end])?;
        Some(Mark {
            on: on.to_owned(),
            state,
            rest: rest[end..].to_owned(),
        })
    }
}

/// Everything a `<whiteboard>` block lists, keyed by alias number.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// The whiteboard's `DocId`.
    pub id: String,
    /// Its title.
    pub title: String,
    /// `fN` → frame.
    pub frames: BTreeMap<u32, Frame>,
    /// `cN` → card.
    pub cards: BTreeMap<u32, Card>,
    /// `lN` → link.
    pub links: BTreeMap<u32, Link>,
    /// `mN` → mark.
    pub marks: BTreeMap<u32, Mark>,
}

/// `c12` with prefix `c` → 12.
fn num(s: &str, prefix: char) -> Option<u32> {
    let d = s.strip_prefix(prefix)?;
    if d.is_empty() || !d.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    d.parse().ok()
}

/// The alias and the rest of a line starting `<prefix>N `.
fn alias_line(line: &str, prefix: char) -> Option<(u32, &str)> {
    let (a, rest) = line.split_once(' ')?;
    Some((num(a, prefix)?, rest))
}

/// The value of `name="…"` in an open tag, entity-decoded.
fn attr(tag: &str, name: &str) -> Option<String> {
    let key = format!(" {name}=\"");
    let start = tag.find(&key)? + key.len();
    let end = tag[start..].find('"')? + start;
    Some(
        tag[start..end]
            .replace("&quot;", "\"")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&"),
    )
}

/// Escape for an attribute value (`&quot; &amp; &lt; &gt;`).
fn attr_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The QSTR of §3.2: quoted, `\` and `"` escaped, newlines as spaces, cut to `max` chars with `…`.
pub fn quote(s: &str, max: usize) -> String {
    let flat: String = s
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    let cut: String = if flat.chars().count() > max {
        let mut t: String = flat.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    } else {
        flat
    };
    let mut out = String::with_capacity(cut.len() + 2);
    out.push('"');
    for c in cut.chars() {
        if c == '\\' || c == '"' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// A leading QSTR: `(text, rest after the closing quote)`.
pub fn unquote(s: &str) -> Option<(String, &str)> {
    let body = s.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = body.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => out.push(chars.next()?.1),
            '"' => return Some((out, &body[i + 1..])),
            c => out.push(c),
        }
    }
    None
}

/// Parse a `<whiteboard …>` block (not a `brief` line). `None` when `text` is not one.
pub fn parse_outline(text: &str) -> Option<Facts> {
    let mut lines = text.lines();
    let open = lines.next()?;
    if !open.starts_with("<whiteboard id=\"") || open.contains(" brief=\"true\"") {
        return None;
    }
    let mut f = Facts {
        id: attr(open, "id")?,
        title: attr(open, "title").unwrap_or_default(),
        ..Facts::default()
    };
    let mut container = Place::Loose;
    let mut rows: HashMap<u32, (Place, u32)> = HashMap::new();
    for line in lines {
        if line == "</whiteboard>" {
            break;
        }
        if let Some(rest) = line.strip_prefix("frame ") {
            if let Some((n, rest)) = alias_line(rest, 'f')
                && let Some(fr) = Frame::parse(rest)
            {
                f.frames.insert(n, fr);
                container = Place::Frame(n);
            }
        } else if line == "loose" {
            container = Place::Loose;
        } else if let Some(rest) = line.strip_prefix("  row ") {
            if let Some((r, cards)) = rest.split_once(": ")
                && let Ok(r) = r.parse::<u32>()
            {
                for a in cards.split(' ') {
                    if let Some(n) = num(a, 'c') {
                        rows.insert(n, (container, r));
                    }
                }
            }
        } else if let Some((n, rest)) = alias_line(line, 'c') {
            if let Some(c) = Card::parse(rest) {
                f.cards.insert(n, c);
            }
        } else if let Some((n, rest)) = alias_line(line, 'l') {
            if let Some(l) = Link::parse(rest) {
                f.links.insert(n, l);
            }
        } else if let Some((n, rest)) = alias_line(line, 'm')
            && let Some(rest) = rest.strip_prefix("on ")
            && let Some(m) = Mark::parse(rest)
        {
            f.marks.insert(n, m);
        }
    }
    for (n, c) in &mut f.cards {
        c.at = rows.get(n).copied();
    }
    Some(f)
}

// ---------------------------------------------------------------------------------------------
// Changes: the diff of two fact sets, rendered and parsed.
// ---------------------------------------------------------------------------------------------

/// Sort key of a change line: (`-` 0 / `~` 1 / `+` 2, item order f c l m, alias number, sequence).
type Key = (u8, u8, u32, u32);

fn kind_rank(prefix: char) -> u8 {
    match prefix {
        't' => 0, // `~title`
        'f' => 1,
        'c' => 2,
        'l' => 3,
        _ => 4,
    }
}

/// The `+`/`-`/`~` lines that turn `old` into `new`, in output order (§3.2: `-`, `~`, `+`, each by alias).
pub fn diff(old: &Facts, new: &Facts) -> Vec<String> {
    let mut out: Vec<(Key, String)> = Vec::new();
    let mut push = |sign: u8, prefix: char, n: u32, line: String| {
        let seq = out.len() as u32;
        out.push(((sign, kind_rank(prefix), n, seq), line));
    };
    if old.title != new.title {
        push(1, 't', 0, format!("~title {}", quote(&new.title, 80)));
    }
    // Frames.
    for (n, fr) in &new.frames {
        match old.frames.get(n) {
            None => push(2, 'f', *n, format!("+{}", fr.line(*n))),
            Some(o) => {
                if o.title != fr.title {
                    push(1, 'f', *n, format!("~f{n} retitled {}", quote(&fr.title, 80)));
                }
                if o.preset != fr.preset {
                    let p = fr.preset.as_deref().unwrap_or("none");
                    push(1, 'f', *n, format!("~f{n} preset {p}"));
                }
            }
        }
    }
    for n in old.frames.keys().filter(|n| !new.frames.contains_key(n)) {
        push(0, 'f', *n, format!("-f{n} removed"));
    }
    // Cards.
    for (n, c) in &new.cards {
        let Some(o) = old.cards.get(n) else {
            let mut line = format!("+{}", c.line(*n));
            if let Some((place, row)) = c.at {
                let _ = write!(line, "{SEP}in {}", place.render(row));
            }
            push(2, 'c', *n, line);
            continue;
        };
        let filled = o.state == CardState::Placeholder && c.state != CardState::Placeholder;
        if filled {
            push(1, 'c', *n, format!("~c{n} filled v{}", c.versions.unwrap_or(1)));
        } else if o.versions != c.versions
            && let (Some(a), Some(b)) = (o.versions, c.versions)
        {
            push(1, 'c', *n, format!("~c{n} v{a} → v{b}"));
        }
        let state_changed = o.state != c.state || o.tag != c.tag;
        match c.state {
            CardState::Dimmed if state_changed => push(
                1,
                'c',
                *n,
                format!("~c{n} dimmed {}", quote(c.tag.as_deref().unwrap_or(""), 120)),
            ),
            CardState::Live if state_changed && !filled => push(1, 'c', *n, format!("~c{n} live")),
            _ => {}
        }
        if o.title != c.title {
            push(1, 'c', *n, format!("~c{n} retitled {}", quote(&c.title, 80)));
        }
        if o.at != c.at
            && let (Some((op, or)), Some((np, nr))) = (o.at, c.at)
        {
            push(
                1,
                'c',
                *n,
                format!("~c{n} moved: {} → {}", op.render(or), np.render(nr)),
            );
        }
        if o.extras != c.extras {
            // The extras ride along so applying the line heals the baseline (as-built B2).
            push(1, 'c', *n, format!("~c{n} edited{}", c.extras));
        }
    }
    for n in old.cards.keys().filter(|n| !new.cards.contains_key(n)) {
        push(0, 'c', *n, format!("-c{n} archived"));
    }
    // Links.
    for (n, l) in &new.links {
        match old.links.get(n) {
            None => push(2, 'l', *n, format!("+{}", l.line(*n))),
            Some(o) => {
                if (o.from, &o.arrow, o.to) != (l.from, &l.arrow, l.to) {
                    push(1, 'l', *n, format!("~l{n} now {}", l.ends()));
                }
                if o.label != l.label {
                    push(
                        1,
                        'l',
                        *n,
                        format!("~l{n} relabelled {}", quote(l.label.as_deref().unwrap_or(""), 80)),
                    );
                }
            }
        }
    }
    for n in old.links.keys().filter(|n| !new.links.contains_key(n)) {
        push(0, 'l', *n, format!("-l{n} removed"));
    }
    // Marks.
    for (n, m) in &new.marks {
        match old.marks.get(n) {
            None => push(2, 'm', *n, format!("+{}", m.line(*n))),
            Some(o) if o.state != m.state => {
                push(1, 'm', *n, format!("~m{n} {}", m.state.as_str()));
            }
            Some(_) => {}
        }
    }
    for n in old.marks.keys().filter(|n| !new.marks.contains_key(n)) {
        push(0, 'm', *n, format!("-m{n} answered"));
    }
    out.sort_by_key(|(k, _)| *k);
    out.into_iter().map(|(_, l)| l).collect()
}

/// Apply one `+`/`-`/`~` line to `f`. `false` when the line does not parse (it is skipped).
pub fn apply_line(f: &mut Facts, line: &str) -> bool {
    apply_inner(f, line).is_some()
}

fn apply_inner(f: &mut Facts, line: &str) -> Option<()> {
    if let Some(rest) = line.strip_prefix('+') {
        if let Some(rest) = rest.strip_prefix("frame ") {
            let (n, rest) = alias_line(rest, 'f')?;
            f.frames.insert(n, Frame::parse(rest)?);
        } else if let Some((n, rest)) = alias_line(rest, 'c') {
            let (body, at) = match rest.rfind(" · in ") {
                Some(i) => match Place::parse(&rest[i + " · in ".len()..]) {
                    Some(at) => (&rest[..i], Some(at)),
                    None => (rest, None),
                },
                None => (rest, None),
            };
            let mut c = Card::parse(body)?;
            c.at = at;
            f.cards.insert(n, c);
        } else if let Some((n, rest)) = alias_line(rest, 'l') {
            f.links.insert(n, Link::parse(rest)?);
        } else {
            let (n, rest) = alias_line(rest, 'm')?;
            f.marks.insert(n, Mark::parse(rest.strip_prefix("on ")?)?);
        }
        return Some(());
    }
    if let Some(rest) = line.strip_prefix('-') {
        let (a, what) = rest.split_once(' ')?;
        match (a.chars().next()?, what) {
            ('c', "archived") => f.cards.remove(&num(a, 'c')?).map(|_| ()),
            ('l', "removed") => f.links.remove(&num(a, 'l')?).map(|_| ()),
            ('f', "removed") => f.frames.remove(&num(a, 'f')?).map(|_| ()),
            ('m', "answered") => f.marks.remove(&num(a, 'm')?).map(|_| ()),
            _ => None,
        }?;
        return Some(());
    }
    let rest = line.strip_prefix('~')?;
    if let Some(t) = rest.strip_prefix("title ") {
        f.title = unquote(t)?.0;
        return Some(());
    }
    let (a, what) = rest.split_once(' ')?;
    match a.chars().next()? {
        'c' => {
            let c = f.cards.get_mut(&num(a, 'c')?)?;
            if let Some(v) = what.strip_prefix("filled v") {
                c.state = CardState::Live;
                c.tag = None;
                if c.versioned() {
                    c.versions = Some(v.parse().ok()?);
                }
            } else if let Some(t) = what.strip_prefix("dimmed ") {
                c.state = CardState::Dimmed;
                c.tag = Some(unquote(t)?.0);
            } else if what == "live" {
                c.state = CardState::Live;
                c.tag = None;
            } else if let Some(t) = what.strip_prefix("retitled ") {
                c.title = unquote(t)?.0;
            } else if let Some(m) = what.strip_prefix("moved: ") {
                let (_, to) = m.split_once(" → ")?;
                c.at = Some(Place::parse(to)?);
            } else if let Some(x) = what.strip_prefix("edited") {
                c.extras = x.to_owned();
            } else {
                let (_, b) = what.split_once(" → ")?;
                c.versions = Some(num(b, 'v')?);
            }
        }
        'l' => {
            let l = f.links.get_mut(&num(a, 'l')?)?;
            if let Some(t) = what.strip_prefix("relabelled ") {
                let label = unquote(t)?.0;
                l.label = (!label.is_empty()).then_some(label);
            } else {
                let (from, arrow, to, tail) = Link::parse_ends(what.strip_prefix("now ")?)?;
                if !tail.is_empty() {
                    return None;
                }
                (l.from, l.arrow, l.to) = (from, arrow, to);
            }
        }
        'm' => {
            f.marks.get_mut(&num(a, 'm')?)?.state = MarkState::parse(what)?;
        }
        'f' => {
            let fr = f.frames.get_mut(&num(a, 'f')?)?;
            if let Some(t) = what.strip_prefix("retitled ") {
                fr.title = unquote(t)?.0;
            } else {
                let p = what.strip_prefix("preset ")?;
                fr.preset = (p != "none").then(|| p.to_owned());
            }
        }
        _ => return None,
    }
    Some(())
}

/// The baseline of one whiteboard in a transcript (the notebook pattern, `notebook_context.rs`).
pub struct Baseline;

impl Baseline {
    /// Scan `texts` in order: a full `<whiteboard id="ID"` block resets the facts, a
    /// `<whiteboard_changes whiteboard="ID"` note applies its lines once a block was seen;
    /// anything else (brief lines included) is ignored. `None`: no block in the transcript.
    pub fn of<S: AsRef<str>>(texts: &[S], id: &str) -> Option<Facts> {
        let block = format!("<whiteboard id=\"{id}\"");
        let changes = format!("<whiteboard_changes whiteboard=\"{id}\"");
        let mut facts: Option<Facts> = None;
        for t in texts {
            let t = t.as_ref();
            if t.starts_with(&block) {
                if let Some(f) = parse_outline(t) {
                    facts = Some(f);
                }
            } else if t.starts_with(&changes)
                && let Some(f) = facts.as_mut()
            {
                for line in t.lines().skip(1) {
                    if line.starts_with(['+', '-', '~']) {
                        apply_line(f, line);
                    }
                }
            }
        }
        facts
    }
}

/// The `in view:` / `selected:` / `image K:` / `images omitted:` lines of a send context, with
/// ULIDs mapped to the aliases of the fresh outline (unknown ones are left out).
pub fn context_lines(
    ctx: &WbSendContext,
    aliases: &BTreeMap<String, String>,
    facts: Option<&Facts>,
) -> Vec<String> {
    let alias = |id: &String| aliases.get(id).cloned();
    let mut out = Vec::new();
    let in_view: Vec<String> = ctx
        .in_view
        .iter()
        .filter_map(alias)
        .map(|a| match (num(&a, 'f'), facts) {
            (Some(n), Some(f)) => match f.frames.get(&n) {
                Some(fr) => format!("{a} {}", quote(&fr.title, 80)),
                None => a,
            },
            _ => a,
        })
        .collect();
    if !in_view.is_empty() {
        out.push(format!("in view: {}", in_view.join(", ")));
    }
    let selected: Vec<String> = ctx.selected.iter().filter_map(alias).collect();
    if !selected.is_empty() {
        out.push(format!("selected: {}", selected.join(", ")));
    }
    for (k, img) in ctx.images.iter().enumerate() {
        let on = match &img.card {
            Some(c) => match alias(c) {
                Some(a) => a,
                None => continue,
            },
            None => "canvas".to_owned(),
        };
        let marks: Vec<String> = img.marks.iter().filter_map(alias).collect();
        let mut line = format!("image {}: {on}", k + 1);
        if !marks.is_empty() {
            let _ = write!(line, "{SEP}marks {}", marks.join(" "));
        }
        out.push(line);
    }
    if ctx.omitted > 0 {
        let why = match ctx.omitted_why.as_deref() {
            Some("no vision") => "no vision",
            _ => "budget",
        };
        out.push(format!("images omitted: {} ({why})", ctx.omitted));
    }
    out
}

/// A `<whiteboard_changes>` note: context lines, then at most [`MAX_CHANGES`] change lines and the
/// overflow line. `None` when there is nothing to say.
pub fn render_changes(id: &str, context: &[String], changes: &[String]) -> Option<String> {
    if context.is_empty() && changes.is_empty() {
        return None;
    }
    let mut s = format!("<whiteboard_changes whiteboard=\"{id}\">\n");
    for l in context {
        s.push_str(l);
        s.push('\n');
    }
    for l in changes.iter().take(MAX_CHANGES) {
        s.push_str(l);
        s.push('\n');
    }
    if changes.len() > MAX_CHANGES {
        let _ = writeln!(
            s,
            "… {} more changes — wb_read for the full outline",
            changes.len() - MAX_CHANGES
        );
    }
    s.push_str("</whiteboard_changes>");
    Some(s)
}

/// The one-line stand-in for a whiteboard beyond the first [`MAX_FULL`] (not a baseline).
pub fn brief_line(id: &str, title: &str, cards: u32) -> String {
    format!(
        "<whiteboard id=\"{id}\" title=\"{}\" cards=\"{cards}\" brief=\"true\"/>",
        attr_escape(title)
    )
}

/// One bound whiteboard as the injection site sees it this turn.
#[derive(Clone, Debug)]
pub struct Bound {
    /// `DocId`.
    pub doc: String,
    /// `DocMeta.title`.
    pub title: String,
    /// Listed cards.
    pub cards: u32,
    /// `DocMeta.updated_ms`.
    pub updated_ms: i64,
    /// The fresh `<whiteboard>` block.
    pub text: String,
    /// ULID → alias.
    pub aliases: BTreeMap<String, String>,
    /// What the owner's send carried, when this turn is that send's.
    pub context: Option<WbSendContext>,
}

/// The user-role messages of this turn for the bound whiteboards, in order (pure; §5 injection
/// steps 2–3). `history` is the session's transcript texts, oldest first.
///
/// The [`MAX_FULL`] whiteboards with a pending send context first, then the newest `updated_ms`,
/// get the full block (no baseline yet) or a change note; every other one gets one brief line,
/// once per context (all brief lines share one message).
pub fn turn_messages<S: AsRef<str>>(bound: &[Bound], history: &[S]) -> Vec<String> {
    let mut order: Vec<&Bound> = bound.iter().collect();
    order.sort_by(|a, b| {
        b.context
            .is_some()
            .cmp(&a.context.is_some())
            .then(b.updated_ms.cmp(&a.updated_ms))
            .then(a.doc.cmp(&b.doc))
    });
    let mut out = Vec::new();
    let mut briefs = Vec::new();
    for (i, b) in order.into_iter().enumerate() {
        if i >= MAX_FULL {
            let seen = format!("<whiteboard id=\"{}\"", b.doc);
            let already = history
                .iter()
                .any(|t| t.as_ref().lines().any(|l| l.starts_with(&seen)));
            if !already {
                briefs.push(brief_line(&b.doc, &b.title, b.cards));
            }
            continue;
        }
        let fresh = parse_outline(&b.text);
        let ctx = b
            .context
            .as_ref()
            .map(|c| context_lines(c, &b.aliases, fresh.as_ref()))
            .unwrap_or_default();
        match (Baseline::of(history, &b.doc), fresh.as_ref()) {
            (Some(base), Some(fresh)) => {
                let changes = diff(&base, fresh);
                out.extend(render_changes(&b.doc, &ctx, &changes));
            }
            _ => {
                out.push(b.text.clone());
                out.extend(render_changes(&b.doc, &ctx, &[]));
            }
        }
    }
    if !briefs.is_empty() {
        out.push(briefs.join("\n"));
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Runtime: send context cache, turn boundaries, the injection call and the RPC handlers.
// ---------------------------------------------------------------------------------------------

/// Pending send contexts: (root session or `None`, whiteboard) → (stored at, context).
static CONTEXTS: LazyLock<Mutex<HashMap<(Option<SessionId>, String), (Instant, WbSendContext)>>> =
    LazyLock::new(Default::default);

/// Whiteboards whose turn `WbTurn { Begin }` opened, per session, for the matching `End`.
static BEGUN: LazyLock<Mutex<HashMap<SessionId, Vec<String>>>> = LazyLock::new(Default::default);

/// Keep `ctx` for the next injection of `root` (any session when `None`) for `whiteboard`.
pub fn put_context(root: Option<SessionId>, whiteboard: &str, ctx: WbSendContext) {
    let mut map = CONTEXTS.lock().unwrap_or_else(|p| p.into_inner());
    map.retain(|_, (at, _)| at.elapsed() < CONTEXT_TTL);
    map.insert((root, whiteboard.to_owned()), (Instant::now(), ctx));
}

/// Take the pending context of `root` for `whiteboard`, else the session-less one.
pub fn take_context(root: SessionId, whiteboard: &str) -> Option<WbSendContext> {
    let mut map = CONTEXTS.lock().unwrap_or_else(|p| p.into_inner());
    map.retain(|_, (at, _)| at.elapsed() < CONTEXT_TTL);
    map.remove(&(Some(root), whiteboard.to_owned()))
        .or_else(|| map.remove(&(None, whiteboard.to_owned())))
        .map(|(_, c)| c)
}

/// An error from the document actor that means "stop binding this one".
fn gone(e: &str) -> bool {
    e.contains("not a whiteboard") || e.contains("no such document") || e.contains("not found")
}

/// What the document store says about a marker's id, for [`marker_refusal`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkerDoc {
    /// The owner may read it: its kind and whether it is in the trash.
    Readable {
        /// Its kind is `DocKind::Whiteboard`.
        whiteboard: bool,
        /// It is in the trash.
        trashed: bool,
    },
    /// The lookup failed (missing, unreadable, another engine serves it, …).
    Unreadable(String),
}

/// 218 M5: why a `[attached whiteboard …]` marker must not bind, or `None` when it may: the
/// input is the owner's own, and the id is a readable whiteboard that is not in the trash.
pub fn marker_refusal(owner: bool, doc: &MarkerDoc) -> Option<String> {
    if !owner {
        return Some("the input is not the owner's".into());
    }
    match doc {
        MarkerDoc::Readable { whiteboard: false, .. } => Some("not a whiteboard".into()),
        MarkerDoc::Readable { trashed: true, .. } => Some("in the trash".into()),
        MarkerDoc::Readable { .. } => None,
        MarkerDoc::Unreadable(e) => Some(e.clone()),
    }
}

/// What the owner may read of `id`: `MetaOf` as the owner (access and kind), then `WbInfo` (trash).
async fn marker_doc(docs: &kameo::actor::ActorRef<document::DocumentActor>, id: &str) -> MarkerDoc {
    let doc = document::DocId(id.to_owned());
    let meta = match docs
        .ask(document::MetaOf {
            doc: doc.clone(),
            by: document::Principal::Owner { device: None },
        })
        .await
    {
        Ok(m) => m,
        Err(e) => return MarkerDoc::Unreadable(e.to_string()),
    };
    if meta.kind != document::DocKind::Whiteboard {
        return MarkerDoc::Readable {
            whiteboard: false,
            trashed: false,
        };
    }
    match docs.ask(document::wb::WbInfo { doc }).await {
        Ok(info) => MarkerDoc::Readable {
            whiteboard: true,
            trashed: info.trashed,
        },
        Err(e) => MarkerDoc::Unreadable(e.to_string()),
    }
}

/// What [`bound`] found for this turn.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Gate {
    /// The session is bound to at least one whiteboard.
    pub any: bool,
    /// The owner's markers whose whiteboard this engine cannot read: `(id, why)` (218 M6).
    pub unreadable: Vec<(String, String)>,
}

impl Gate {
    /// Anything for [`inject`] to do.
    pub fn open(&self) -> bool {
        self.any || !self.unreadable.is_empty()
    }
}

/// §5 (a): bind the whiteboards the input's markers name (`by = 'marker'`) — only when `owner`
/// (every input of the turn is the owner's own) and the id is a readable, live whiteboard; a
/// refused marker is logged, never bound (218 M5); an owner's marker this engine cannot read is
/// reported for a notice (M6). Then answer whether `session` is bound to any whiteboard — the
/// cheap gate before [`inject`] reads the transcript.
pub async fn bound(store: &Store, session: SessionId, input: &str, owner: bool) -> Gate {
    let refs: Vec<_> = crate::attach::parse(crate::clock::strip(input))
        .refs
        .into_iter()
        .filter(|r| r.kind == crate::attach::Kind::Whiteboard)
        .collect();
    let handle = document::Handle::lookup();
    let mut gate = Gate::default();
    for r in refs {
        let doc = match (&handle, owner) {
            (Some(h), true) => marker_doc(h.actor(), &r.id).await,
            (None, true) => MarkerDoc::Unreadable("documents are disabled".into()),
            (_, false) => MarkerDoc::Unreadable(String::new()),
        };
        if let Some(why) = marker_refusal(owner, &doc) {
            tracing::warn!(%session, whiteboard = %r.id, "whiteboard marker not bound: {why}");
            if owner && let MarkerDoc::Unreadable(e) = &doc {
                gate.unreadable.push((r.id.clone(), plain(e)));
            }
            continue;
        }
        if let Err(e) = store.bind_whiteboard(session, &r.id, "marker").await {
            tracing::warn!(%session, whiteboard = %r.id, "whiteboard bind: {e:#}");
        }
    }
    gate.any = match store.whiteboards_of(session).await {
        Ok(rows) => !rows.is_empty(),
        Err(e) => {
            tracing::warn!(%session, "whiteboards_of: {e:#}");
            false
        }
    };
    gate
}

/// 218 M6: the one-line stand-in for a bound whiteboard whose outline this engine cannot read
/// (another engine serves it, or the read failed). Not a baseline; said once per context.
pub fn unavailable_line(id: &str, why: &str) -> String {
    let why: String = why
        .chars()
        .map(|c| if c == '\n' || c == '"' || c == '<' || c == '>' { ' ' } else { c })
        .take(160)
        .collect();
    format!(
        "<whiteboard_unavailable id=\"{id}\" reason=\"{why}\"/> This chat is bound to that whiteboard, \
         but its outline cannot be read here, so the wb_* functions will fail too. Tell the user, and \
         suggest opening this chat on the engine that serves the whiteboard."
    )
}

/// The [`unavailable_line`]s not yet said in this context (`history`), in order.
pub fn unavailable_messages<S: AsRef<str>>(failed: &[(String, String)], history: &[S]) -> Vec<String> {
    failed
        .iter()
        .filter(|(id, _)| {
            let seen = format!("<whiteboard_unavailable id=\"{id}\"");
            !history.iter().any(|t| t.as_ref().starts_with(&seen))
        })
        .map(|(id, why)| unavailable_line(id, why))
        .collect()
}

/// A document error in plain words: `NotHub` → "another engine serves it".
fn plain(e: &str) -> String {
    if e.contains("NotHub") || e.contains("hub lease") || e.contains("not the hub") {
        "another engine serves it".to_owned()
    } else {
        e.to_owned()
    }
}

/// §5 injection for one turn of `session` (round 0 of a chat turn): for every bound whiteboard
/// open its turn and fetch its outline, and return the messages to persist (user role), in order.
/// Errors skip a whiteboard; a document that is no longer a whiteboard, or is trashed, is unbound.
pub async fn inject<S: AsRef<str>>(
    store: &Store,
    session: SessionId,
    history: &[S],
    unreadable: Vec<(String, String)>,
) -> Vec<String> {
    let mut failed = unreadable;
    let rows = match store.whiteboards_of(session).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(%session, "whiteboards_of: {e:#}");
            Vec::new()
        }
    };
    let handle = document::Handle::lookup();
    let root = store.root_of(session).await.unwrap_or(session);
    let mut bound = Vec::new();
    let mut begun = Vec::new();
    for (wb, _) in rows {
        let Some(handle) = &handle else { break };
        let docs = handle.actor();
        let doc = document::DocId(wb.clone());
        match docs.ask(document::wb::WbInfo { doc: doc.clone() }).await {
            Ok(info) if info.trashed => {
                let _ = store.unbind_whiteboard(session, &wb).await;
                continue;
            }
            Ok(_) => {}
            Err(e) => {
                let e = e.to_string();
                if gone(&e) {
                    let _ = store.unbind_whiteboard(session, &wb).await;
                } else {
                    tracing::warn!(%session, whiteboard = %wb, "whiteboard outline unavailable: {e}");
                    failed.push((wb.clone(), plain(&e)));
                }
                continue;
            }
        }
        if docs
            .ask(document::wb::WbTurn {
                doc: doc.clone(),
                session: session.0,
                phase: document::wb::TurnPhase::Begin,
            })
            .await
            .is_ok()
        {
            begun.push(wb.clone());
        }
        match docs.ask(document::wb::WbOutline { doc, frame: None }).await {
            Ok(r) => bound.push(Bound {
                context: take_context(root, &wb),
                doc: wb,
                title: r.title,
                cards: r.cards,
                updated_ms: r.updated_ms,
                text: r.text,
                aliases: r.aliases,
            }),
            Err(e) => {
                let e = e.to_string();
                tracing::warn!(%session, whiteboard = %wb, "whiteboard outline unavailable: {e}");
                failed.push((wb, plain(&e)));
            }
        }
    }
    if !begun.is_empty() {
        BEGUN
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(session, begun);
    }
    let mut out = turn_messages(&bound, history);
    out.extend(unavailable_messages(&failed, history));
    out
}

/// §5 step 4: the turn of `session` finished — `WbTurn { End }` for every whiteboard its injection
/// began, detached and best effort; never blocks the turn.
pub fn end_turn(session: SessionId) {
    let Some(docs) = BEGUN
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(&session)
    else {
        return;
    };
    let Some(handle) = document::Handle::lookup() else {
        return;
    };
    let actor = handle.actor().clone();
    tokio::spawn(async move {
        for wb in docs {
            let r = actor
                .ask(document::wb::WbTurn {
                    doc: document::DocId(wb.clone()),
                    session: session.0,
                    phase: document::wb::TurnPhase::End,
                })
                .await;
            if let Err(e) = r {
                tracing::debug!(%session, whiteboard = %wb, "whiteboard turn end: {e}");
            }
        }
    });
}

/// `ask` → the store `session` lives in (a private tree's own, else the engine's); `None` = the engine's.
#[derive(Clone, Copy, Debug)]
pub struct WbStore(pub Option<SessionId>);

impl kameo::message::Message<WbStore> for crate::Agents {
    type Reply = Store;

    async fn handle(
        &mut self,
        m: WbStore,
        _: &mut kameo::message::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        match m.0 {
            Some(s) => self.store_for(s).clone(),
            None => self.store.clone(),
        }
    }
}

/// The store for `session` through the agent actor.
async fn store_of(session: Option<SessionId>) -> Result<Store, String> {
    let agents = rpc_core::lookup::<crate::Agents>(crate::AGENT_ACTOR).map_err(|e| e.message().to_owned())?;
    agents.ask(WbStore(session)).await.map_err(|e| e.to_string())
}

/// The document hook (§3.5): `wb_*` writes, `wb_create` and `show_whiteboard` bind their session,
/// `by = 'tool'`. Installed once, beside the insight hooks.
pub fn install_hooks() {
    document::wb::hooks::install(document::wb::hooks::Hooks {
        bind: std::sync::Arc::new(|session, doc| {
            let session = SessionId(session);
            tokio::spawn(async move {
                let r = match store_of(Some(session)).await {
                    Ok(store) => store
                        .bind_whiteboard(session, &doc.0, "tool")
                        .await
                        .map_err(|e| format!("{e:#}")),
                    Err(e) => Err(e),
                };
                if let Err(e) = r {
                    tracing::warn!(%session, whiteboard = %doc.0, "whiteboard tool bind: {e}");
                }
            });
        }),
    });
}

/// RPC handlers behind `rpc_whiteboards` (218 §4.2).
pub mod rpc {
    use super::store_of;
    use crate::rpc_whiteboards::{WbSendContext, WhiteboardSession};
    use jsonrpsee::core::RpcResult;
    use rpc_core::failed;
    use runtime::SessionId;

    fn fail(e: impl std::fmt::Display) -> jsonrpsee::types::ErrorObjectOwned {
        failed(e.to_string())
    }

    /// Check through `WbInfo` that `whiteboard` is a live whiteboard.
    async fn check(whiteboard: &document::DocId) -> RpcResult<()> {
        let docs = document::rpc_support::documents()?;
        let info = document::rpc_support::doc_send(
            &docs,
            document::wb::WbInfo {
                doc: whiteboard.clone(),
            },
        )
        .await?;
        if info.trashed {
            return Err(failed(format!("{} is in the trash", whiteboard.0)));
        }
        Ok(())
    }

    /// `whiteboard_bind`.
    pub async fn bind(session: SessionId, whiteboard: document::DocId) -> RpcResult<()> {
        check(&whiteboard).await?;
        let store = store_of(Some(session)).await.map_err(fail)?;
        if store.session(session).await.map_err(fail)?.is_none() {
            return Err(failed(format!("no such session {session}")));
        }
        store
            .bind_whiteboard(session, &whiteboard.0, "composer")
            .await
            .map_err(fail)?;
        Ok(())
    }

    /// `whiteboard_unbind`.
    pub async fn unbind(session: SessionId, whiteboard: document::DocId) -> RpcResult<()> {
        let store = store_of(Some(session)).await.map_err(fail)?;
        store
            .unbind_whiteboard(session, &whiteboard.0)
            .await
            .map_err(fail)?;
        Ok(())
    }

    /// `whiteboard_sessions`: newest binding first; rows of sessions that no longer exist go.
    pub async fn sessions(whiteboard: document::DocId) -> RpcResult<Vec<WhiteboardSession>> {
        let store = store_of(None).await.map_err(fail)?;
        let rows = store
            .sessions_of_whiteboard(&whiteboard.0)
            .await
            .map_err(fail)?;
        let mut out = Vec::with_capacity(rows.len());
        for (session, bound_ms) in rows {
            match store.session(session).await.map_err(fail)? {
                Some(s) => out.push(WhiteboardSession {
                    session,
                    title: s.title,
                    bound_ms,
                }),
                None => {
                    let _ = store.forget_whiteboard_session(session).await;
                }
            }
        }
        Ok(out)
    }

    /// `session_whiteboards`.
    pub async fn of_session(session: SessionId) -> RpcResult<Vec<document::DocId>> {
        let store = store_of(Some(session)).await.map_err(fail)?;
        Ok(store
            .whiteboards_of(session)
            .await
            .map_err(fail)?
            .into_iter()
            .map(|(wb, _)| document::DocId(wb))
            .collect())
    }

    /// `whiteboard_send_context`.
    pub async fn send_context(
        session: Option<SessionId>,
        whiteboard: document::DocId,
        context: WbSendContext,
    ) -> RpcResult<()> {
        let root = match session {
            Some(s) => Some(
                store_of(Some(s))
                    .await
                    .map_err(fail)?
                    .root_of(s)
                    .await
                    .map_err(fail)?,
            ),
            None => None,
        };
        super::put_context(root, &whiteboard.0, context);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
