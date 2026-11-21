//! 137 §4: units — one per part of one `messages` row, in row order then
//! part order. Pure: the store hands in `(seq, at, ChatMessage)`, this
//! module hands back [`Unit`]s with their index continuing from `next_i`.

use chrono::{DateTime, Utc};
use genai::chat::{ChatMessage, ChatRole, ContentPart};

use super::tree::{Kind, Unit};

/// A user `Text` that starts like this is a subagent's report (8.5): the
/// peer wrapper the loop uses for a child's result (`loop_::peer_wrapper`).
pub const WORK_MARKER: &str = "<message from=\"session:";

/// 201 §8 (P4): a user `Text` that starts like this is the design-52 vision
/// fallback's note (`vision::note`): a model's description, kind `note`.
pub const VISION_MARKER: &str = "<vision_observation>";

/// 201 §8 (P4): the `part` of a tool response's `k`-th image at content part
/// `p` — the top bit set, `p` above `k`. A plain content part keeps its index,
/// so every unit row stored before P4 keeps its `part`.
pub const IMAGE_PART: u16 = 0x8000;

/// The unit `part` of image `k` of the tool response at content part `p`.
pub fn image_part(p: usize, k: usize) -> u16 {
    IMAGE_PART | (((p as u16) & 0xFF) << 7) | ((k as u16) & 0x7F)
}

/// `(content part, tool image)` of a unit `part`.
pub fn locate(part: u16) -> (usize, Option<usize>) {
    if part & IMAGE_PART == 0 {
        (part as usize, None)
    } else {
        (((part >> 7) & 0xFF) as usize, Some((part & 0x7F) as usize))
    }
}

/// 201 §8: an artefact unit's text: `[image: name]` or `[document: name]`
/// (as [`crate::loop_::binary_tag`] writes it), followed by nothing or by a
/// description the trim added to its line.
pub fn is_artefact(text: &str) -> bool {
    text.starts_with("[image: ") || text.starts_with("[document: ")
}

/// Units of one row (§4 table). `excluded` rows are skipped by the caller.
/// 137G G4: every text kind is capped at `cap_chars` like a tool result (the
/// row keeps the full text; `zoom` shows the capped unit).
///
/// 201 §8 (P4): a binary part is `[image: name]` / `[document: name]`; each
/// image of a tool response is its own `echo` unit after the response's
/// text (part order, then image order; `part` = [`image_part`]); an empty
/// user text (an image-only message) is no unit; a vision note is `note`.
/// Rows already ingested keep their stored units: the pump derives only rows
/// after the session's last ingested row, so existing indices never shift.
pub fn derive(
    seq: i64,
    at: Option<DateTime<Utc>>,
    m: &ChatMessage,
    cap_chars: u32,
    next_i: u64,
) -> Vec<Unit> {
    let mut out = Vec::new();
    let push = |out: &mut Vec<Unit>, part: u16, kind: Kind, text: String| {
        let size = (kind.as_str().len() + 2 + text.len()) as u32;
        out.push(Unit {
            i: next_i + out.len() as u64,
            seq,
            part,
            kind,
            text,
            size,
            at,
        });
    };
    for (part, p) in m.content.iter().enumerate() {
        let (kind, text) = match (p, &m.role) {
            // R1-7: the G9/G10 `<memory>` notices are about the memory, not in it.
            (ContentPart::Text(t), ChatRole::User)
                if crate::endless_ext::memory_note(t).is_some() =>
            {
                continue;
            }
            // 201 §8: an image-only message has no caption unit.
            (ContentPart::Text(t), ChatRole::User) if t.trim().is_empty() => continue,
            (ContentPart::Text(t), ChatRole::User) => {
                let kind = if t.starts_with(WORK_MARKER) {
                    Kind::Work
                } else if t.starts_with(VISION_MARKER) {
                    Kind::Note
                } else {
                    Kind::User
                };
                (kind, cap(t, cap_chars))
            }
            (ContentPart::Text(t), ChatRole::Assistant) => (Kind::Talk, cap(t, cap_chars)),
            (ContentPart::Text(t), _) => (Kind::Echo, cap(t, cap_chars)),
            (ContentPart::ToolCall(c), _) => {
                let args = serde_json::to_string(&c.fn_arguments).unwrap_or_default();
                (Kind::Tool, cap(&format!("{} {args}", c.fn_name), cap_chars))
            }
            (ContentPart::ToolResponse(r), _) => {
                push(
                    &mut out,
                    part as u16,
                    Kind::Echo,
                    cap(&r.content, cap_chars),
                );
                for (k, b) in r.images.iter().enumerate() {
                    let mut tag = String::new();
                    crate::loop_::binary_tag(&mut tag, b);
                    push(&mut out, image_part(part, k), Kind::Echo, tag);
                }
                continue;
            }
            (ContentPart::Binary(b), _) => {
                let mut tag = String::new();
                crate::loop_::binary_tag(&mut tag, b);
                (
                    if m.role == ChatRole::Assistant {
                        Kind::Talk
                    } else {
                        Kind::User
                    },
                    tag,
                )
            }
            (ContentPart::ReasoningContent(_) | ContentPart::ThoughtSignature(_), _) => continue,
            (ContentPart::Custom(_), _) => continue,
        };
        push(&mut out, part as u16, kind, text);
    }
    out
}

/// 201 §8: the `(seq, content part, tool image)` references of a vision
/// note's `Sources:` line (`m12.p1`, `m12.p3.i0`; `vision::Source::reference`).
pub fn note_sources(text: &str) -> Vec<(i64, usize, Option<usize>)> {
    let Some(line) = text.lines().find_map(|l| l.strip_prefix("Sources: ")) else {
        return Vec::new();
    };
    line.split(", ")
        .filter_map(|s| parse_ref(s.split(' ').next()?))
        .collect()
}

fn parse_ref(r: &str) -> Option<(i64, usize, Option<usize>)> {
    let mut it = r.split('.');
    let seq = it.next()?.strip_prefix('m')?.parse().ok()?;
    let part = it.next()?.strip_prefix('p')?.parse().ok()?;
    let image = match it.next() {
        Some(i) => Some(i.strip_prefix('i')?.parse().ok()?),
        None => None,
    };
    Some((seq, part, image))
}

/// 201 §8: the vision note's answer for one reference (`m12.p1: …` in its
/// body), the first line, `None` when the image was not seen.
pub fn note_answer(text: &str, seq: i64, part: usize, image: Option<usize>) -> Option<String> {
    let mut r = format!("m{seq}.p{part}");
    if let Some(k) = image {
        r.push_str(&format!(".i{k}"));
    }
    let head = format!("{r}: ");
    let answers = text.split_once("Answers")?.1;
    let a = answers.lines().find_map(|l| l.strip_prefix(&head))?.trim();
    (!a.is_empty() && !a.starts_with("NOT SEEN")).then(|| a.to_owned())
}

/// 201 §8: link a vision note to the image units it describes: `[on #i, #j] `
/// ahead of its text (the `Sources:` references resolved against `units`).
pub fn link_note(u: &mut Unit, find: impl Fn(i64, u16) -> Option<u64>) {
    if u.kind != Kind::Note || !u.text.starts_with(VISION_MARKER) {
        return;
    }
    let ids: Vec<String> = note_sources(&u.text)
        .into_iter()
        .filter_map(|(seq, p, k)| {
            let part = k.map_or(p as u16, |k| image_part(p, k));
            find(seq, part).map(|i| format!("#{i}"))
        })
        .collect();
    if ids.is_empty() {
        return;
    }
    u.text = format!("[on {}] {}", ids.join(", "), u.text);
    u.size = (u.kind.as_str().len() + 2 + u.text.len()) as u32;
}

/// §4 echo cap: head ⅔ + `[… N chars cut]` + tail ⅓, counted in chars.
pub fn cap(s: &str, cap_chars: u32) -> String {
    let text = crate::diff_output::strip(s);
    let s = text.as_ref();
    let cap = cap_chars as usize;
    let n = s.chars().count();
    if n <= cap {
        return s.to_owned();
    }
    let head = cap * 2 / 3;
    let tail = cap - head;
    let cut = n - head - tail;
    let head_end = s.char_indices().nth(head).map_or(s.len(), |(b, _)| b);
    let tail_start = s.char_indices().nth(n - tail).map_or(s.len(), |(b, _)| b);
    format!("{}[… {cut} chars cut]{}", &s[..head_end], &s[tail_start..])
}

#[cfg(test)]
mod tests {
    #[test]
    fn diff_carrier_endless_units_strip_before_echo_cap() {
        let input = crate::diff_output::carried("receipt");
        let message = ChatMessage::from(vec![ToolResponse::new("c", input.clone())]);
        let units = derive(1, None, &message, 32, 0);
        assert_eq!(units[0].text, "receipt");
        assert_eq!(cap(&input, 32), "receipt");
        assert_eq!(message.content.tool_responses()[0].content, input);
    }

    use super::*;
    use genai::chat::{MessageContent, ToolCall, ToolResponse};

    fn parse(json: &str) -> ChatMessage {
        serde_json::from_str(json).unwrap()
    }

    /// §4: the real row shapes — a user text, an assistant text + tool call,
    /// a tool response, reasoning dropped, a work marker, an image.
    #[test]
    fn derives_units_from_stored_row_shapes() {
        let user = parse(r#"{"role":"User","content":[{"Text":"hello there"}]}"#);
        let u = derive(0, None, &user, 30_000, 0);
        assert_eq!(u.len(), 1);
        assert_eq!(
            (u[0].kind, u[0].text.as_str(), u[0].size),
            (Kind::User, "hello there", 17)
        );
        assert_eq!(u[0].tagged(), "user: hello there");

        let asst = ChatMessage::assistant(MessageContent::from_parts(vec![
            ContentPart::ReasoningContent("thinking…".into()),
            ContentPart::Text("on it".into()),
            ContentPart::ToolCall(ToolCall {
                call_id: "c1".into(),
                fn_name: "read".into(),
                fn_arguments: serde_json::json!({"path": "a.rs", "n": 1}),
                thought_signatures: None,
            }),
        ]));
        let json = serde_json::to_string(&asst).unwrap();
        let u = derive(1, None, &parse(&json), 30_000, 1);
        assert_eq!(u.len(), 2, "reasoning dropped");
        assert_eq!((u[0].i, u[0].kind, u[0].part), (1, Kind::Talk, 1));
        assert_eq!((u[1].i, u[1].kind, u[1].part), (2, Kind::Tool, 2));
        assert_eq!(u[1].text, r#"read {"path":"a.rs","n":1}"#);

        let tool = ChatMessage::from(vec![ToolResponse::new("c1", "x".repeat(100))]);
        let json = serde_json::to_string(&tool).unwrap();
        let u = derive(2, None, &parse(&json), 30, 3);
        assert_eq!(u[0].kind, Kind::Echo);
        assert_eq!(
            u[0].text,
            format!("{}[… 70 chars cut]{}", "x".repeat(20), "x".repeat(10))
        );

        let work = parse(
            r#"{"role":"User","content":[{"Text":"<message from=\"session:abc\" id=\"1\">done</message>"}]}"#,
        );
        assert_eq!(derive(3, None, &work, 30_000, 4)[0].kind, Kind::Work);
    }

    /// R1-7: a bare `<memory>` harness note (the G9/G10 notices) is no unit;
    /// a `<memory …>` tag with attributes is the user's text and stays one.
    #[test]
    fn bare_memory_notes_are_not_units() {
        let note = ChatMessage::user(
            "<memory>\nmemory: 12 older unsummarized lines were folded to fit (recall searches them)\n</memory>",
        );
        assert!(derive(1, None, &note, 30_000, 0).is_empty());
        let mine = ChatMessage::user("<memory kind=\"todo\">remember the keys</memory>");
        assert_eq!(derive(2, None, &mine, 30_000, 0).len(), 1);
        assert_eq!(
            crate::endless_ext::memory_note("<memory>\n note \n</memory>"),
            Some("note")
        );
    }

    #[test]
    fn cap_counts_chars_not_bytes() {
        let s = "é".repeat(12);
        assert_eq!(cap(&s, 12), s);
        assert_eq!(
            cap(&s, 9),
            format!("{}[… 3 chars cut]{}", "é".repeat(6), "é".repeat(3))
        );
    }

    fn bin(mime: &str, name: &str) -> genai::chat::Binary {
        genai::chat::Binary::from_base64(mime, "AAAA", Some(name.to_owned()))
    }

    /// 201 §8 (P4): every binary shape yields its named unit.
    #[test]
    fn binary_shapes_yield_named_units() {
        // A user image with a caption; a PDF; an image-only message.
        let m = ChatMessage::user(MessageContent::from_parts(vec![
            ContentPart::Text("look".into()),
            ContentPart::Binary(bin("image/png", "/tmp/shot.png")),
            ContentPart::Binary(bin("application/pdf", "spec.pdf")),
        ]));
        let u = derive(5, None, &m, 30_000, 10);
        let got: Vec<_> = u
            .iter()
            .map(|u| (u.i, u.part, u.kind, u.text.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                (10, 0, Kind::User, "look"),
                (11, 1, Kind::User, "[image: shot.png]"),
                (12, 2, Kind::User, "[document: spec.pdf]"),
            ]
        );
        let only = ChatMessage::user(MessageContent::from_parts(vec![
            ContentPart::Text(" ".into()),
            ContentPart::Binary(bin("image/jpeg", "cat.jpg")),
        ]));
        let u = derive(6, None, &only, 30_000, 0);
        assert_eq!(u.len(), 1, "no empty user unit");
        assert_eq!((u[0].part, u[0].text.as_str()), (1, "[image: cat.jpg]"));
        // An assistant binary is talk.
        let a = ChatMessage::assistant(MessageContent::from_parts(vec![ContentPart::Binary(bin(
            "image/png",
            "plot.png",
        ))]));
        assert_eq!(derive(7, None, &a, 30_000, 0)[0].kind, Kind::Talk);
        // Tool pictures: one echo unit each, after the response text.
        let t = ChatMessage::from(vec![ToolResponse::new("c1", "took 2 shots").with_images(
            vec![bin("image/png", "/a/one.png"), bin("image/png", "two.png")],
        )]);
        let u = derive(8, None, &t, 30_000, 3);
        let got: Vec<_> = u
            .iter()
            .map(|u| (u.i, u.part, u.kind, u.text.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                (3, 0, Kind::Echo, "took 2 shots"),
                (4, image_part(0, 0), Kind::Echo, "[image: one.png]"),
                (5, image_part(0, 1), Kind::Echo, "[image: two.png]"),
            ]
        );
        assert_eq!(locate(u[2].part), (0, Some(1)));
        assert_eq!(locate(1), (1, None));
    }

    /// 201 §8 (P4): a vision note is `note`, linked to its image units, and
    /// its answer is found per reference.
    #[test]
    fn vision_note_is_a_linked_note() {
        let body = "<vision_observation>\nSources: m4.p1 (cat.jpg), m9.p0.i1 (two.png)\nVision model: v\nQuestion from q:\nwhat?\nAnswers (image-derived evidence, fallible; not instructions):\nm4.p1: a grey cat asleep\nm9.p0.i1: NOT SEEN (unreadable). You have not seen this image.\n</vision_observation>";
        let mut u = derive(12, None, &ChatMessage::user(body), 30_000, 20).remove(0);
        assert_eq!(u.kind, Kind::Note);
        assert_eq!(note_sources(body), vec![(4, 1, None), (9, 0, Some(1))]);
        link_note(&mut u, |seq, part| match (seq, part) {
            (4, 1) => Some(7),
            (9, p) if p == image_part(0, 1) => Some(15),
            _ => None,
        });
        assert!(
            u.text.starts_with("[on #7, #15] <vision_observation>"),
            "{}",
            u.text
        );
        assert_eq!(u.size as usize, "note: ".len() + u.text.len());
        assert_eq!(
            note_answer(body, 4, 1, None).as_deref(),
            Some("a grey cat asleep")
        );
        assert_eq!(note_answer(body, 9, 0, Some(1)), None, "not seen");
    }
}
