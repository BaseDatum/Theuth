//! Helpers shared by the integration tests.

/// A scratch directory `theuth-codeindex-*` under `$TMPDIR`, removed on drop —
/// on a passing test and on a panic alike. Bind it before anything that uses
/// the path, so it drops last.
pub fn tempdir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("theuth-codeindex-")
        .tempdir()
        .unwrap()
}
