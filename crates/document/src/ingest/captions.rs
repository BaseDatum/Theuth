// 144-W1 owns this file.
//! YouTube captions without the network (§4.1.1 step 1): the video id of a URL, the `captionTracks`
//! array and `videoDetails` out of a watch page's embedded player JSON, the track to take for the
//! configured languages, and the words of a `json3` track. [`super::routes::youtube`] does the fetching.

use serde::Deserialize;

/// `youtube.com/watch?v=<id>`, `youtu.be/<id>`, `/shorts/<id>`, `/embed/<id>`, `/live/<id>`;
/// the id is validated (`[A-Za-z0-9_-]{11}`) before anything is spawned with it (§15).
pub fn video_id(url: &str) -> Option<String> {
    let u = url::Url::parse(url.trim()).ok()?;
    let host = u
        .host_str()?
        .trim_start_matches("www.")
        .trim_start_matches("m.");
    let id = match host {
        "youtu.be" => u.path_segments()?.next()?.to_owned(),
        "youtube.com" | "music.youtube.com" | "youtube-nocookie.com" => {
            let mut segs = u.path_segments()?;
            match segs.next()? {
                "watch" => u.query_pairs().find(|(k, _)| k == "v")?.1.into_owned(),
                "shorts" | "embed" | "live" | "v" => segs.next()?.to_owned(),
                _ => return None,
            }
        }
        _ => return None,
    };
    valid_id(&id).then_some(id)
}

/// Eleven URL-safe base64 chars.
pub fn valid_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// One entry of `captionTracks`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Track {
    /// Timed-text URL (add `&fmt=json3`).
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    /// BCP-47 (`en`, `en-US`, `pt-PT`).
    #[serde(rename = "languageCode", default)]
    pub lang: String,
    /// `"asr"` for auto-generated tracks.
    #[serde(default)]
    pub kind: Option<String>,
}

/// What the page says about the video.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Details {
    pub title: Option<String>,
    pub channel: Option<String>,
    pub length_ms: Option<u64>,
}

/// The `captionTracks` array of a watch page; empty when the page has none (no captions, consent
/// wall, age gate).
pub fn tracks(html: &str) -> Vec<Track> {
    json_after(html, "\"captionTracks\":")
        .and_then(|s| serde_json::from_str::<Vec<Track>>(s).ok())
        .unwrap_or_default()
}

/// `videoDetails` of a watch page.
pub fn details(html: &str) -> Details {
    #[derive(Deserialize)]
    struct Raw {
        title: Option<String>,
        author: Option<String>,
        #[serde(rename = "lengthSeconds")]
        length_seconds: Option<String>,
    }
    let Some(raw) =
        json_after(html, "\"videoDetails\":").and_then(|s| serde_json::from_str::<Raw>(s).ok())
    else {
        return Details::default();
    };
    Details {
        title: raw.title,
        channel: raw.author,
        length_ms: raw
            .length_seconds
            .and_then(|s| s.parse::<u64>().ok())
            .map(|s| s * 1000),
    }
}

/// The track for `langs` in preference order (`en` matches `en-US`; a human track beats `asr`),
/// else the first.
pub fn pick<'a>(tracks: &'a [Track], langs: &[String]) -> Option<&'a Track> {
    for want in langs {
        let want = want.to_ascii_lowercase();
        let matching = tracks.iter().filter(|t| {
            let l = t.lang.to_ascii_lowercase();
            l == want || l.split('-').next() == Some(want.as_str())
        });
        if let Some(t) = matching
            .clone()
            .find(|t| t.kind.as_deref() != Some("asr"))
            .or_else(|| matching.clone().next())
        {
            return Some(t);
        }
    }
    tracks.first()
}

/// `(start ms, text)` of every word-ish segment of a `json3` track.
pub fn json3_words(json: &str) -> anyhow::Result<Vec<(u64, String)>> {
    #[derive(Deserialize)]
    struct Seg {
        utf8: String,
        #[serde(rename = "tOffsetMs", default)]
        offset: u64,
    }
    #[derive(Deserialize)]
    struct Event {
        #[serde(rename = "tStartMs", default)]
        start: u64,
        #[serde(default)]
        segs: Vec<Seg>,
    }
    #[derive(Deserialize)]
    struct Json3 {
        #[serde(default)]
        events: Vec<Event>,
    }
    let j: Json3 = serde_json::from_str(json)?;
    Ok(j.events
        .into_iter()
        .flat_map(|e| {
            e.segs
                .into_iter()
                .filter(|s| !s.utf8.trim().is_empty())
                .map(move |s| (e.start + s.offset, s.utf8))
        })
        .collect())
}

/// The balanced JSON value (object or array) that follows `marker` in `html`.
fn json_after<'a>(html: &'a str, marker: &str) -> Option<&'a str> {
    let start = html.find(marker)? + marker.len();
    let bytes = html.as_bytes();
    let mut i = start;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    let open = *bytes.get(i)?;
    let close = match open {
        b'[' => b']',
        b'{' => b'}',
        _ => return None,
    };
    let (mut depth, mut in_str, mut esc) = (0usize, false, false);
    for (j, &b) in bytes.iter().enumerate().skip(i) {
        if in_str {
            match b {
                _ if esc => esc = false,
                b'\\' => esc = true,
                b'"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            _ if b == open => depth += 1,
            _ if b == close => {
                depth -= 1;
                if depth == 0 {
                    return html.get(i..=j);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_ids_from_the_url_shapes_and_nothing_else() {
        for u in [
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=1s",
            "https://youtu.be/dQw4w9WgXcQ?si=x",
            "https://m.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://www.youtube.com/shorts/dQw4w9WgXcQ",
            "https://www.youtube.com/embed/dQw4w9WgXcQ",
        ] {
            assert_eq!(video_id(u).as_deref(), Some("dQw4w9WgXcQ"), "{u}");
        }
        assert_eq!(video_id("https://www.youtube.com/@channel"), None);
        assert_eq!(video_id("https://youtu.be/short"), None);
        assert_eq!(video_id("https://example.com/watch?v=dQw4w9WgXcQ"), None);
        assert_eq!(video_id("https://www.youtube.com/watch?v=../../etc"), None);
    }

    #[test]
    fn picks_the_human_track_of_the_wanted_language() {
        let t = |lang: &str, kind: Option<&str>| Track {
            base_url: format!("https://x/{lang}{}", kind.unwrap_or("")),
            lang: lang.into(),
            kind: kind.map(str::to_owned),
        };
        let tracks = [t("de", None), t("en", Some("asr")), t("en-US", None)];
        assert_eq!(
            pick(&tracks, &["en".into()]).unwrap().base_url,
            "https://x/en-US"
        );
        assert_eq!(
            pick(&tracks, &["fr".into(), "de".into()]).unwrap().base_url,
            "https://x/de"
        );
        assert_eq!(
            pick(&tracks, &["fr".into()]).unwrap().base_url,
            "https://x/de"
        );
        assert!(pick(&[], &["en".into()]).is_none());
    }

    #[test]
    fn a_saved_watch_page_yields_tracks_and_details() {
        let html = include_str!("../../tests/fixtures/ingest/watch.html");
        let t = tracks(html);
        assert_eq!(t.len(), 3);
        assert_eq!(t[1].kind.as_deref(), Some("asr"));
        assert_eq!(pick(&t, &["en".into()]).unwrap().lang, "en-GB");
        assert!(
            pick(&t, &["en".into()])
                .unwrap()
                .base_url
                .ends_with("&lang=en-GB")
        );
        let d = details(html);
        assert_eq!(d.title.as_deref(), Some("Failover in practice"));
        assert_eq!(d.channel.as_deref(), Some("Ops Channel"));
        assert_eq!(d.length_ms, Some(2_461_000));
        assert!(tracks("<html>no player json</html>").is_empty());
    }

    #[test]
    fn json3_words_become_minute_blocks() {
        let words =
            json3_words(include_str!("../../tests/fixtures/ingest/captions.json3")).unwrap();
        assert_eq!(words.len(), 12);
        assert_eq!(words[1], (520, " to".into()));
        let md = crate::ingest::timed_blocks(words, 60_000);
        assert_eq!(
            md,
            "## [00:00]\n\nWelcome to the talk. Still first minute.\n\n## [01:01]\n\nSecond minute begins.\n\n## [03:20]\n\nMuch later.\n\n"
        );
    }
}
