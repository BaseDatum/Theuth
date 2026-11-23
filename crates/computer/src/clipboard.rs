//! The clipboard tools (design doc 161 §5) over one long-lived
//! `arboard::Clipboard` on one worker thread for the process ([`Workers`],
//! never idle-retired). On X11 the selection must keep being served after a
//! call returns; the long-lived instance replaces agent-desktop's
//! clipboard-owner process, whose commands theuth never calls.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::RgbaImage;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::{Risk, Tool, ToolCx, ToolOutput};

use crate::png;
use crate::tools::{Ctx, computer_tool, free_png, shot_dir, try_out};
use crate::worker::{DEADLINE, Failure, Workers};

/// The process's clipboard thread; the `arboard::Clipboard` is opened by
/// the first call and kept for the life of the thread.
pub struct Clipboard {
    workers: Workers<(), Option<arboard::Clipboard>>,
}

impl Default for Clipboard {
    fn default() -> Self {
        Self {
            workers: Workers::new(
                |()| "computer-clipboard".to_owned(),
                DEADLINE,
                None,
                |_, _| {},
            ),
        }
    }
}

impl Clipboard {
    /// Run `op` on the clipboard thread; a missed deadline abandons the
    /// thread and the next call starts a fresh one.
    async fn run<R: Send + 'static>(
        &self,
        op: impl FnOnce(&mut arboard::Clipboard) -> Result<R, String> + Send + 'static,
    ) -> Result<R, String> {
        let ran = self
            .workers
            .run(&(), move |slot| {
                let clipboard = match slot {
                    Some(c) => c,
                    None => slot.insert(
                        arboard::Clipboard::new()
                            .map_err(|e| format!("clipboard unavailable: {e}"))?,
                    ),
                };
                op(clipboard)
            })
            .await;
        match ran {
            Ok(r) => r,
            Err(Failure::Timeout) => Err(format!(
                "the clipboard did not answer within {} s; its thread was restarted",
                DEADLINE.as_secs()
            )),
            Err(Failure::Panicked(m)) => Err(format!("clipboard thread recovered: {m}")),
            Err(Failure::Gone(m)) => Err(m),
        }
    }
}

/// What `computer_clipboard_read` reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClipFormat {
    /// Plain text (default).
    #[default]
    Text,
    /// An image, attached to the result.
    Image,
    /// Copied files, one path per line.
    Files,
}

/// Arguments for [`ClipboardReadTool`].
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ClipboardReadArgs {
    /// `text` (default), `image` or `files`.
    #[serde(default)]
    pub format: Option<ClipFormat>,
}

/// Arguments for [`ClipboardWriteTool`]: exactly one of the four.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ClipboardWriteArgs {
    /// Text to put on the clipboard.
    #[serde(default)]
    pub text: Option<String>,
    /// Absolute path of a PNG to put on the clipboard as an image.
    #[serde(default)]
    pub image_path: Option<PathBuf>,
    /// Absolute paths of files to put on the clipboard, as a file manager copies them.
    #[serde(default)]
    pub files: Option<Vec<PathBuf>>,
    /// `true` empties the clipboard.
    #[serde(default)]
    pub clear: bool,
}

/// A validated write.
#[derive(Debug, PartialEq, Eq)]
enum Write {
    Text(String),
    Image(PathBuf),
    Files(Vec<PathBuf>),
    Clear,
}

/// Exactly one of text, image_path, a non-empty files list or clear; paths
/// must be absolute (161 W-h), since a tool call has no working directory.
fn write_op(a: ClipboardWriteArgs) -> Result<Write, String> {
    match (a.text, a.image_path, a.files, a.clear) {
        (Some(text), None, None, false) => Ok(Write::Text(text)),
        (None, Some(path), None, false) => absolute(path).map(Write::Image),
        (None, None, Some(files), false) if !files.is_empty() => files
            .into_iter()
            .map(absolute)
            .collect::<Result<_, _>>()
            .map(Write::Files),
        (None, None, None, true) => Ok(Write::Clear),
        _ => {
            Err("give exactly one of text, image_path, files (non-empty) or clear: true".to_owned())
        }
    }
}

fn absolute(path: PathBuf) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(format!("{} is not an absolute path", path.display()))
    }
}

/// `arboard`'s read error in the model's words.
fn read_error(what: &str) -> impl FnOnce(arboard::Error) -> String + '_ {
    move |e| match e {
        arboard::Error::ContentNotAvailable => format!("the clipboard holds no {what}"),
        e => format!("clipboard read failed: {e}"),
    }
}

/// The clipboard image as a PNG under the long-edge cap, like a screenshot
/// (161 W-h), and the provider's size cap; with its encoded size.
fn encode(img: arboard::ImageData<'_>, max_edge: u32) -> Result<(Vec<u8>, (u32, u32)), String> {
    let (w, h) = (
        u32::try_from(img.width).map_err(|e| e.to_string())?,
        u32::try_from(img.height).map_err(|e| e.to_string())?,
    );
    let rgba = RgbaImage::from_raw(w, h, img.bytes.into_owned())
        .ok_or("clipboard image has the wrong size for its pixels")?;
    png::bounded(&rgba, max_edge)
}

/// A PNG file as `arboard` image data.
fn load_png(path: &Path) -> Result<arboard::ImageData<'static>, String> {
    let img = image::open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .to_rgba8();
    let (w, h) = img.dimensions();
    Ok(arboard::ImageData {
        width: w as usize,
        height: h as usize,
        bytes: Cow::Owned(img.into_raw()),
    })
}

computer_tool!(
    /// `computer_clipboard_read`: text, an attached image or file paths.
    ClipboardReadTool, "computer_clipboard_read", Risk::ReadOnly, ClipboardReadArgs,
    "Read the clipboard: `format` text (default), image (attached to this result) or files (one path per line).",
    |ctx, a, cx| {
        let text = match a.format.unwrap_or_default() {
            ClipFormat::Text => {
                try_out!(ctx.clipboard.run(|c| c.get_text().map_err(read_error("text"))).await)
            }
            ClipFormat::Files => {
                let files = try_out!(
                    ctx.clipboard.run(|c| c.get().file_list().map_err(read_error("files"))).await
                );
                files.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n")
            }
            ClipFormat::Image => {
                let max_edge = ctx.cfg.max_edge;
                let (png, (w, h)) = try_out!(
                    ctx.clipboard
                        .run(move |c| encode(c.get_image().map_err(read_error("image"))?, max_edge))
                        .await
                );
                let dir = shot_dir(&cx.resources, cx.session);
                tokio::fs::create_dir_all(&dir).await?;
                let path = free_png(&dir, "clip")?;
                tokio::fs::write(&path, &png).await?;
                return Ok(ToolOutput {
                    files: vec![path],
                    ..ToolOutput::ok(format!("clipboard image {w}\u{d7}{h}"))
                });
            }
        };
        Ok(ToolOutput::ok(text))
    }
);

computer_tool!(
    /// `computer_clipboard_write`: exactly one of text, a PNG, files or clear.
    ClipboardWriteTool, "computer_clipboard_write", Risk::Mutating, ClipboardWriteArgs,
    "Set the clipboard to exactly one of: `text`, the PNG at `image_path`, the `files` (absolute paths, as a file manager copies them), or nothing with `clear: true`.",
    |ctx, a, _cx| {
        // 210 §7.2: a pending human-only card could be answered by a paste of the agent's
        // making (an Allow link, a typed reply); the clipboard is off limits until it is.
        try_out!(crate::tools::refuse_while_human_only().await);
        let op = try_out!(write_op(a));
        let done = match &op {
            Write::Text(t) => format!("copied {} chars", t.chars().count()),
            Write::Image(p) => format!("copied image {}", p.display()),
            Write::Files(f) => format!("copied {} files", f.len()),
            Write::Clear => "cleared the clipboard".to_owned(),
        };
        try_out!(
            ctx.clipboard
                .run(move |c| {
                    match op {
                        Write::Text(t) => c.set_text(t),
                        Write::Image(p) => c.set_image(load_png(&p)?),
                        Write::Files(f) => c.set().file_list(&f),
                        Write::Clear => c.clear(),
                    }
                    .map_err(|e| format!("clipboard write failed: {e}"))
                })
                .await
        );
        Ok(ToolOutput::ok(done))
    }
);

#[cfg(test)]
mod tests {
    use super::*;

    fn args(
        text: Option<&str>,
        image: Option<&str>,
        files: Option<Vec<&str>>,
        clear: bool,
    ) -> ClipboardWriteArgs {
        ClipboardWriteArgs {
            text: text.map(str::to_owned),
            image_path: image.map(PathBuf::from),
            files: files.map(|f| f.into_iter().map(PathBuf::from).collect()),
            clear,
        }
    }

    #[test]
    fn write_takes_exactly_one_target() {
        assert_eq!(
            write_op(args(Some("hi"), None, None, false)),
            Ok(Write::Text("hi".into()))
        );
        assert_eq!(write_op(args(None, None, None, true)), Ok(Write::Clear));
        let abs = if cfg!(windows) { "C:\\x.png" } else { "/x.png" };
        assert_eq!(
            write_op(args(None, Some(abs), None, false)),
            Ok(Write::Image(abs.into()))
        );
        assert_eq!(
            write_op(args(None, None, Some(vec![abs, abs]), false)),
            Ok(Write::Files(vec![abs.into(), abs.into()]))
        );
        for bad in [
            args(None, None, None, false),
            args(Some("hi"), None, None, true),
            args(Some("hi"), Some(abs), None, false),
            args(None, None, Some(Vec::new()), false),
        ] {
            assert!(write_op(bad).is_err());
        }
    }

    #[test]
    fn write_refuses_relative_paths() {
        assert!(write_op(args(None, Some("x.png"), None, false)).is_err());
        let abs = if cfg!(windows) { "C:\\a" } else { "/a" };
        assert!(write_op(args(None, None, Some(vec![abs, "b"]), false)).is_err());
    }

    #[test]
    fn encode_caps_the_long_edge_and_keeps_small_images() {
        let img = |w: usize, h: usize| arboard::ImageData {
            width: w,
            height: h,
            bytes: Cow::Owned(vec![255; w * h * 4]),
        };
        let (png, size) = encode(img(400, 100), 200).unwrap();
        assert_eq!(size, (200, 50));
        assert_eq!(
            image::load_from_memory(&png)
                .unwrap()
                .into_rgba8()
                .dimensions(),
            (200, 50)
        );
        assert_eq!(encode(img(30, 20), 200).unwrap().1, (30, 20));
        let short = arboard::ImageData {
            width: 4,
            height: 4,
            bytes: Cow::Owned(vec![0; 3]),
        };
        assert!(encode(short, 200).is_err());
    }
}
