//! The input thread: one `std::thread` owning the process's single [`enigo::Enigo`],
//! driven through a channel (27 §3.4). Also the key-chord parser and the refused-chord check.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use enigo::{Axis, Coordinate, Direction, Key, Keyboard, Mouse};
use tokio::sync::oneshot;

/// Mouse button, as the model names it.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Button {
    /// Primary button.
    Left,
    /// Secondary button.
    Right,
    /// Wheel button.
    Middle,
}

impl From<Button> for enigo::Button {
    fn from(b: Button) -> Self {
        match b {
            Button::Left => enigo::Button::Left,
            Button::Right => enigo::Button::Right,
            Button::Middle => enigo::Button::Middle,
        }
    }
}

/// One request to the input thread. Points are global logical coordinates.
#[derive(Debug)]
pub enum Action {
    /// Move the pointer.
    Move(i32, i32),
    /// Click `count` times (with `mods` held), optionally moving first.
    Click {
        /// Where to move before clicking.
        at: Option<(i32, i32)>,
        /// Which button.
        button: Button,
        /// 1 = click, 2 = double, 3 = triple; 0 counts as 1.
        count: u8,
        /// Modifier keys held across the clicks.
        mods: Vec<Key>,
    },
    /// Press at `from`, move to `to` in steps, release.
    Drag {
        /// Start point.
        from: (i32, i32),
        /// End point.
        to: (i32, i32),
        /// Which button.
        button: Button,
    },
    /// Scroll in line units; positive = down / right.
    Scroll {
        /// Where to move before scrolling.
        at: Option<(i32, i32)>,
        /// Horizontal lines.
        dx: i32,
        /// Vertical lines.
        dy: i32,
    },
    /// Type text through the keyboard layout.
    Type(String),
    /// Press keys in order, release in reverse.
    Chord(Vec<Key>),
    /// Read the pointer position.
    Location,
}

/// What the thread answers.
#[derive(Debug)]
pub enum Reply {
    /// The action completed.
    Done,
    /// Pointer position in global logical points.
    Location(i32, i32),
}

/// Why an action did not complete.
#[derive(Debug)]
pub enum InputError {
    /// `Enigo::new` reported `NoPermission` (macOS Accessibility not granted).
    NoAccessibility,
    /// enigo refused the event.
    Failed(String),
    /// No reply within 30 s.
    Timeout,
    /// The thread panicked (or is gone); the `Enigo` is rebuilt on the next call.
    Recovered(String),
}

impl std::fmt::Display for InputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoAccessibility => f.write_str("no accessibility permission"),
            Self::Failed(e) => write!(f, "input failed: {e}"),
            Self::Timeout => f.write_str("input thread did not reply within 30 s"),
            Self::Recovered(e) => write!(f, "input thread recovered: {e}"),
        }
    }
}

impl std::error::Error for InputError {}

type Request = (Action, oneshot::Sender<Result<Reply, InputError>>);

/// Clonable sender to the input thread. Dropping every `Handle` ends the thread.
#[derive(Clone)]
pub struct Handle(mpsc::Sender<Request>);

const TIMEOUT: Duration = Duration::from_secs(30);

impl Handle {
    /// Run one action; 30 s deadline.
    pub async fn run(&self, a: Action) -> Result<Reply, InputError> {
        let (tx, rx) = oneshot::channel();
        if self.0.send((a, tx)).is_err() {
            return Err(InputError::Recovered("input thread gone".into()));
        }
        match tokio::time::timeout(TIMEOUT, rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err(InputError::Recovered("input thread gone".into())),
            Err(_) => Err(InputError::Timeout),
        }
    }
}

/// Start the input thread. `Enigo` is built lazily on the first action.
pub fn spawn() -> Handle {
    let (tx, rx) = mpsc::channel::<Request>();
    let spawned = thread::Builder::new()
        .name("computer-input".into())
        .spawn(move || run_loop(rx));
    if let Err(e) = spawned {
        tracing::error!(error = %e, "computer: failed to spawn input thread");
    }
    Handle(tx)
}

fn run_loop(rx: mpsc::Receiver<Request>) {
    let mut enigo: Option<enigo::Enigo> = None;
    while let Ok((action, reply)) = rx.recv() {
        let en = match enigo.as_mut() {
            Some(e) => e,
            None => match enigo::Enigo::new(&enigo::Settings::default()) {
                Ok(e) => enigo.insert(e),
                Err(enigo::NewConError::NoPermission) => {
                    let _ = reply.send(Err(InputError::NoAccessibility));
                    continue;
                }
                Err(e) => {
                    let _ = reply.send(Err(InputError::Failed(e.to_string())));
                    continue;
                }
            },
        };
        let result = catch_unwind(AssertUnwindSafe(|| perform(en, action)));
        let out = match result {
            Ok(r) => r,
            Err(payload) => {
                enigo = None;
                Err(InputError::Recovered(panic_message(payload.as_ref())))
            }
        };
        let _ = reply.send(out);
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".to_owned()
    }
}

fn failed(e: enigo::InputError) -> InputError {
    InputError::Failed(e.to_string())
}

fn perform(en: &mut enigo::Enigo, action: Action) -> Result<Reply, InputError> {
    match action {
        Action::Move(x, y) => en.move_mouse(x, y, Coordinate::Abs).map_err(failed)?,
        Action::Click {
            at,
            button,
            count,
            mods,
        } => {
            if let Some((x, y)) = at {
                en.move_mouse(x, y, Coordinate::Abs).map_err(failed)?;
            }
            press_all(en, &mods)?;
            let btn = enigo::Button::from(button);
            let clicks = (0..count.max(1)).try_for_each(|i| {
                if i > 0 {
                    thread::sleep(Duration::from_millis(80));
                }
                en.button(btn, Direction::Click).map_err(failed)
            });
            release_all(en, &mods);
            clicks?;
        }
        Action::Drag { from, to, button } => {
            let btn = enigo::Button::from(button);
            en.move_mouse(from.0, from.1, Coordinate::Abs)
                .map_err(failed)?;
            en.button(btn, Direction::Press).map_err(failed)?;
            let moved = (1..=16i32).try_for_each(|i| {
                let x = from.0 + (to.0 - from.0) * i / 16;
                let y = from.1 + (to.1 - from.1) * i / 16;
                thread::sleep(Duration::from_millis(16));
                en.move_mouse(x, y, Coordinate::Abs).map_err(failed)
            });
            thread::sleep(Duration::from_millis(50));
            let released = en.button(btn, Direction::Release).map_err(failed);
            moved.and(released)?;
        }
        Action::Scroll { at, dx, dy } => {
            if let Some((x, y)) = at {
                en.move_mouse(x, y, Coordinate::Abs).map_err(failed)?;
            }
            if dy != 0 {
                en.scroll(dy, Axis::Vertical).map_err(failed)?;
            }
            if dx != 0 {
                en.scroll(dx, Axis::Horizontal).map_err(failed)?;
            }
        }
        Action::Type(s) => en.text(&s).map_err(failed)?,
        Action::Chord(keys) => {
            press_all(en, &keys)?;
            release_all(en, &keys);
        }
        Action::Location => {
            let (x, y) = en.location().map_err(failed)?;
            return Ok(Reply::Location(x, y));
        }
    }
    Ok(Reply::Done)
}

/// Press in order; on failure release what was pressed and return the error.
fn press_all(en: &mut enigo::Enigo, keys: &[Key]) -> Result<(), InputError> {
    for (i, k) in keys.iter().enumerate() {
        if let Err(e) = en.key(*k, Direction::Press) {
            release_all(en, &keys[..i]);
            return Err(failed(e));
        }
    }
    Ok(())
}

/// Release in reverse; best effort.
fn release_all(en: &mut enigo::Enigo, keys: &[Key]) {
    for k in keys.iter().rev() {
        if let Err(e) = en.key(*k, Direction::Release) {
            tracing::warn!(key = ?k, error = %e, "computer: key release failed");
        }
    }
}

/// Parse `"cmd+shift+p"` into keys. A trailing `+` (as in `"shift++"`) is the plus key.
pub fn parse_keys(s: &str) -> Result<Vec<Key>, String> {
    let (body, plus) = match s.strip_suffix("++").or_else(|| s.strip_suffix('+')) {
        Some(b) => (b, true),
        None => (s, false),
    };
    let mut keys = Vec::new();
    if !(plus && body.is_empty()) {
        for token in body.split('+') {
            keys.push(parse_key(token)?);
        }
    }
    if plus {
        keys.push(Key::Unicode('+'));
    }
    Ok(keys)
}

fn parse_key(token: &str) -> Result<Key, String> {
    let t = token.trim().to_lowercase();
    let key = match t.as_str() {
        "cmd" | "command" | "super" | "win" | "meta" => Key::Meta,
        "opt" | "option" | "alt" => Key::Alt,
        "ctrl" | "control" => Key::Control,
        "shift" => Key::Shift,
        "enter" | "return" => Key::Return,
        "esc" | "escape" => Key::Escape,
        "tab" => Key::Tab,
        "space" | "spacebar" => Key::Space,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "up" => Key::UpArrow,
        "down" => Key::DownArrow,
        "left" => Key::LeftArrow,
        "right" => Key::RightArrow,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        #[cfg(not(target_os = "macos"))]
        "insert" => Key::Insert,
        "f1" => Key::F1,
        "f2" => Key::F2,
        "f3" => Key::F3,
        "f4" => Key::F4,
        "f5" => Key::F5,
        "f6" => Key::F6,
        "f7" => Key::F7,
        "f8" => Key::F8,
        "f9" => Key::F9,
        "f10" => Key::F10,
        "f11" => Key::F11,
        "f12" => Key::F12,
        "f13" => Key::F13,
        "f14" => Key::F14,
        "f15" => Key::F15,
        "f16" => Key::F16,
        "f17" => Key::F17,
        "f18" => Key::F18,
        "f19" => Key::F19,
        "f20" => Key::F20,
        _ => {
            let mut chars = t.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Key::Unicode(c),
                _ => return Err(format!("unknown key {token:?}")),
            }
        }
    };
    Ok(key)
}

/// Like [`parse_keys`] but only modifier keys are allowed.
pub fn parse_modifiers(s: &str) -> Result<Vec<Key>, String> {
    let keys = parse_keys(s)?;
    if keys
        .iter()
        .all(|k| matches!(k, Key::Meta | Key::Control | Key::Alt | Key::Shift))
    {
        Ok(keys)
    } else {
        Err("modifiers may contain only cmd, ctrl, alt, shift".into())
    }
}

/// The four chords of 27 §0.7, never sent.
const REFUSED: [&[Key]; 4] = [
    &[Key::Meta, Key::Unicode('q')],
    &[Key::Meta, Key::Shift, Key::Unicode('q')],
    &[Key::Meta, Key::Alt, Key::Escape],
    &[Key::Control, Key::Alt, Key::Delete],
];

/// True when `keys` is one of the refused chords, in any order.
pub fn is_refused(keys: &[Key]) -> bool {
    REFUSED
        .iter()
        .any(|r| r.len() == keys.len() && r.iter().all(|k| keys.contains(k)))
}

/// Whether the process may synthesize input (macOS Accessibility). Non-prompting.
#[cfg(target_os = "macos")]
pub fn accessibility_granted() -> bool {
    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }
    // SAFETY: AXIsProcessTrusted takes no arguments, has no preconditions and only reads TCC state.
    unsafe { AXIsProcessTrusted() }
}

/// Whether the process may synthesize input. Always true off macOS (no TCC).
#[cfg(not(target_os = "macos"))]
pub fn accessibility_granted() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_aliases() {
        assert_eq!(
            parse_keys("cmd+shift+p"),
            Ok(vec![Key::Meta, Key::Shift, Key::Unicode('p')])
        );
        assert_eq!(
            parse_keys("shift++"),
            Ok(vec![Key::Shift, Key::Unicode('+')])
        );
        assert_eq!(parse_keys("+"), Ok(vec![Key::Unicode('+')]));
        assert_eq!(parse_keys("F5"), Ok(vec![Key::F5]));
        assert_eq!(
            parse_keys("Control + Return"),
            Ok(vec![Key::Control, Key::Return])
        );
    }

    #[test]
    fn rejects_unknown() {
        assert_eq!(parse_keys("cmdd"), Err("unknown key \"cmdd\"".to_owned()));
        assert_eq!(parse_keys(""), Err("unknown key \"\"".to_owned()));
        assert_eq!(parse_keys("cmd+"), Ok(vec![Key::Meta, Key::Unicode('+')]));
    }

    #[test]
    fn refused_is_order_independent() {
        let k = |s| parse_keys(s).expect("valid chord");
        assert!(is_refused(&k("q+cmd")));
        assert!(is_refused(&k("command+Q")));
        assert!(is_refused(&k("ctrl+alt+delete")));
        assert!(!is_refused(&k("cmd+w")));
        assert!(!is_refused(&k("cmd+q+shift+a")));
    }

    #[test]
    fn modifiers_only() {
        assert_eq!(
            parse_modifiers("cmd+shift"),
            Ok(vec![Key::Meta, Key::Shift])
        );
        assert_eq!(
            parse_modifiers("cmd+p"),
            Err("modifiers may contain only cmd, ctrl, alt, shift".to_owned())
        );
    }
}
