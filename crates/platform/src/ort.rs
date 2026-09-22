//! The ONNX Runtime shared library's file name per OS (58 §0.13).

/// The name Microsoft's release ships as, without its version:
/// `libonnxruntime.dylib`, `onnxruntime.dll`, `libonnxruntime.so`.
pub const fn ort_library_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "libonnxruntime.dylib"
    } else if cfg!(windows) {
        "onnxruntime.dll"
    } else {
        "libonnxruntime.so"
    }
}
