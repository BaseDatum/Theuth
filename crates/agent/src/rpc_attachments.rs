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

/// Validate `ext` and the size cap for its kind, then decode base64 `bytes`
/// (`put_attachment`, `put_session_attachment`).
pub fn decode_attachment(bytes: String, ext: &str) -> RpcResult<Vec<u8>> {
    let cap = match ext {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" => MAX_IMAGE,
        "pdf" => MAX_PDF,
        "m4a" | "mp4" | "webm" | "ogg" | "opus" | "mp3" | "wav" => MAX_AUDIO,
        _ => return Err(failed(format!("unsupported attachment type .{ext}"))),
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(bytes)
        .map_err(|e| failed(format!("bad base64: {e}")))?;
    if bytes.len() > cap {
        return Err(failed(format!(
            "attachment is {} bytes; the cap for .{ext} is {cap}",
            bytes.len()
        )));
    }
    Ok(bytes)
}
