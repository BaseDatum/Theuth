//! Attachment decoding and decoded-size limits shared by agent RPC methods.

use jsonrpsee::core::RpcResult;
use rpc_core::failed;
use base64::Engine as _;

/// Largest decoded image `put_attachment` accepts (17 §1: Anthropic's 5 MB); an
/// `.svg` (a saved diagram) shares the cap.
pub const MAX_IMAGE: usize = 5 * 1024 * 1024;

/// Largest decoded PDF `put_attachment` accepts (20 §2.5: Anthropic's 32 MB).
pub const MAX_PDF: usize = 32 * 1024 * 1024;

/// Largest decoded audio note `put_attachment` accepts (45 §7.1).
pub const MAX_AUDIO: usize = 16 * 1024 * 1024;

/// Largest decoded generic attachment `put_attachment` accepts (same cap as PDF).
pub const MAX_GENERIC: usize = 32 * 1024 * 1024;

/// Longest accepted file extension (ASCII alphanumeric only; empty allowed for
/// extensionless names like `Makefile` or `LICENSE`).
const MAX_EXT_LEN: usize = 16;

/// Whether `ext` is safe to append to a blob filename: empty (extensionless),
/// or a short ASCII alphanumeric token (no `.`, `..`, `/`, `\`, or whitespace).
fn valid_ext(ext: &str) -> bool {
    ext.len() <= MAX_EXT_LEN && ext.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// Validate `ext` and the size cap for its kind, then decode base64 `bytes`
/// (`put_attachment`, `put_session_attachment`).
pub fn decode_attachment(bytes: String, ext: &str) -> RpcResult<Vec<u8>> {
    if !valid_ext(ext) {
        return Err(failed(format!("invalid attachment extension {ext:?}")));
    }
    let cap = match ext.to_ascii_lowercase().as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" => MAX_IMAGE,
        "pdf" => MAX_PDF,
        "m4a" | "mp4" | "webm" | "ogg" | "opus" | "mp3" | "wav" => MAX_AUDIO,
        _ => MAX_GENERIC,
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(bytes)
        .map_err(|e| failed(format!("bad base64: {e}")))?;
    if bytes.len() > cap {
        return Err(failed(if ext.is_empty() {
            format!("attachment is {} bytes; the cap is {cap}", bytes.len())
        } else {
            format!(
                "attachment is {} bytes; the cap for .{ext} is {cap}",
                bytes.len()
            )
        }));
    }
    Ok(bytes)
}
