use super::*;

/// A held paste: large text, or an attached image (17 §4.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Paste {
    /// Text too large for the input box.
    Text(String),
    /// An image file to send with the input.
    Image(Image),
    /// A reference picked with `/attach` (66 §10): its marker line leads the sent text.
    Ref(agent::attach::AttachRef),
}

/// What the input box shows for a held paste: `[⎘ paste 1: 69 lines]`
/// (`chars` for a single long line), `[⎘ image 2: 1440×900]` (file name
/// when the size is unknown; `file` for a PDF), `[⎘ attach 3: <title>]`.
/// `i` is the index into `Model::pastes`.
pub fn paste_token(i: usize, p: &Paste) -> String {
    match p {
        Paste::Text(text) => {
            let lines = text.lines().count();
            let size = if lines > 1 {
                format!("{lines} lines")
            } else {
                format!("{} chars", text.chars().count())
            };
            format!("[⎘ paste {}: {size}]", i + 1)
        }
        Paste::Image(img) if img.w > 0 => format!("[⎘ image {}: {}×{}]", i + 1, img.w, img.h),
        Paste::Image(img) => {
            let name = img.path.file_name().unwrap_or_default().to_string_lossy();
            let kind = if img
                .path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
            {
                "file"
            } else {
                "image"
            };
            format!("[⎘ {kind} {}: {name}]", i + 1)
        }
        Paste::Ref(r) => format!("[⎘ attach {}: {}]", i + 1, r.title),
    }
}

impl Model {

    /// Hold `r` for the next submit; returns its [`paste_token`]. Attaching
    /// the same kind+id twice is a no-op that returns the existing token.
    pub fn attach_ref(&mut self, r: agent::attach::AttachRef) -> String {
        if let Some((i, p)) = self
            .pastes
            .iter()
            .enumerate()
            .find(|(_, p)| matches!(p, Paste::Ref(x) if x.kind == r.kind && x.id == r.id))
        {
            return paste_token(i, p);
        }
        let p = Paste::Ref(r);
        let token = paste_token(self.pastes.len(), &p);
        self.pastes.push(p);
        token
    }
    /// Take a paste: small ones come back verbatim for the input box; large
    /// ones are held in `pastes` and a [`paste_token`] comes back instead.
    /// Terminals paste newlines as `\r`; normalise to `\n`.
    pub fn paste(&mut self, text: &str) -> String {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        if text.lines().count() <= 5 && text.chars().count() <= 400 {
            return text;
        }
        let i = self.pastes.len();
        self.pastes.push(Paste::Text(text));
        paste_token(i, &self.pastes[i])
    }

    /// Hold an image for the next submit; returns its [`paste_token`]. A
    /// text-only shown model gets a notice so the user can `/model` before
    /// Enter (17 §4.4).
    pub fn attach(&mut self, img: Image) -> String {
        self.component_notify(crate::components::Lifecycle::ImageAttached);
        let text_only = self
            .catalog
            .as_ref()
            .and_then(|c| c.closest(&self.model_id))
            .is_some_and(|m| !m.image);
        if text_only {
            let tail = match &self.vision_model {
                Some(v) => format!(
                    "{} will inspect the image for it and answer its question (/vision to change)",
                    v.id
                ),
                None => "the image will be sent as omitted — /model to switch or /vision <model> to set a fallback".to_owned(),
            };
            self.entries.push(Entry::Info(format!(
                "{} is text-only; {tail}",
                self.model_id
            )));
        }
        let p = Paste::Image(img);
        let token = paste_token(self.pastes.len(), &p);
        self.pastes.push(p);
        token
    }

    /// `line` with every paste token the user left in place replaced by its
    /// text, every image token removed (with one adjacent space) and the
    /// images' paths returned in token order, every attach token removed and
    /// its marker line put first (66 §10); `pastes` is emptied either way. A
    /// token the user deleted drops its paste.
    pub(super) fn expand_pastes(&mut self, line: &str) -> (String, Vec<PathBuf>) {
        let mut text = line.to_owned();
        let mut files = Vec::new();
        let mut markers = String::new();
        for (i, p) in std::mem::take(&mut self.pastes).into_iter().enumerate() {
            let token = paste_token(i, &p);
            match p {
                Paste::Text(t) => text = text.replace(&token, &t),
                Paste::Image(_) | Paste::Ref(_) => {
                    let Some(at) = text.find(&token) else {
                        continue;
                    };
                    let end = at + token.len();
                    let range = if text[end..].starts_with(' ') {
                        at..end + 1
                    } else if text[..at].ends_with(' ') {
                        at - 1..end
                    } else {
                        at..end
                    };
                    text.replace_range(range, "");
                    match p {
                        Paste::Image(img) => files.push(img.path),
                        Paste::Ref(r) => {
                            markers.push_str(&agent::attach::marker(&r));
                            markers.push('\n');
                        }
                        Paste::Text(_) => {}
                    }
                }
            }
        }
        if !markers.is_empty() {
            markers.push_str(text.trim_start());
            text = markers;
        }
        (text, files)
    }
}
