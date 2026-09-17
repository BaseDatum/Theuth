use crate::NativeError;

/// Map a displayable error into [`NativeError`].
pub fn to_err<E: std::fmt::Display>(error: E) -> NativeError {
    NativeError::new(error.to_string())
}
