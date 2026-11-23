//! Human-readable sizes and ages, once: every surface prints the same string.

/// `n` bytes in decimal SI units with one decimal (199 UI-2: one unit system), as
/// `bytesize` prints them: `512 B`, `1.5 kB`, `24.1 MB`, `2.0 GB`.
#[must_use]
pub fn bytes(n: u64) -> String {
    bytesize::ByteSize(n).display().si().to_string()
}

/// A span of `secs` seconds, coarse and floored: `48s`, `3m`, `2h`, `3d`.
#[must_use]
pub fn age(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        3600..86_400 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_sizes() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(999), "999 B");
        assert_eq!(bytes(1_000), "1.0 kB");
        assert_eq!(bytes(1_536), "1.5 kB");
        assert_eq!(bytes(83_000), "83.0 kB");
        assert_eq!(bytes(999_949), "999.9 kB");
        // The unit is picked before rounding: just under 1 MB prints as 1000.0 kB.
        assert_eq!(bytes(999_950), "1000.0 kB");
        assert_eq!(bytes(3 * 1024 * 1024), "3.1 MB");
        assert_eq!(bytes(24_100_000), "24.1 MB");
        assert_eq!(bytes(5_000_000_000), "5.0 GB");
        assert_eq!(bytes(u64::MAX), "18.4 EB");
    }

    #[test]
    fn ages_floor_to_one_unit() {
        assert_eq!(age(0), "0s");
        assert_eq!(age(59), "59s");
        assert_eq!(age(60), "1m");
        assert_eq!(age(3_599), "59m");
        assert_eq!(age(7_200), "2h");
        assert_eq!(age(86_400 * 3 + 5), "3d");
    }
}
