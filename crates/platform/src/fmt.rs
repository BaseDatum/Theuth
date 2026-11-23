//! Human-readable sizes and ages, once: every surface prints the same string.

/// `n` bytes in decimal SI units (199 UI-2: one unit system): `512 B`,
/// `1.5 kB`, `24.1 MB`, `2 GB`. At most one decimal.
#[must_use]
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 3] = ["kB", "MB", "GB"];
    if n < 1000 {
        return format!("{n} B");
    }
    #[allow(clippy::cast_precision_loss)] // display only
    let mut v = n as f64 / 1e3;
    let mut unit = 0;
    // `999_950 B` rounds to one decimal as 1000.0: print it in the next unit.
    while v >= 999.95 && unit + 1 < UNITS.len() {
        v /= 1e3;
        unit += 1;
    }
    let s = format!("{v:.1}");
    format!("{} {}", s.strip_suffix(".0").unwrap_or(&s), UNITS[unit])
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
        assert_eq!(bytes(1_000), "1 kB");
        assert_eq!(bytes(1_536), "1.5 kB");
        assert_eq!(bytes(999_949), "999.9 kB");
        assert_eq!(bytes(999_950), "1 MB");
        assert_eq!(bytes(24_100_000), "24.1 MB");
        assert_eq!(bytes(5_000_000_000), "5 GB");
        assert_eq!(bytes(u64::MAX), "18446744073.7 GB");
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
