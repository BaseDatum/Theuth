//! This build's target and the supported-platform contract (58 §0.14).

/// Host and client operating systems the harness knows about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Os {
    Macos,
    Windows,
    Linux,
    Ios,
    Android,
}

/// CPU architectures we ship for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Arch {
    Aarch64,
    X86_64,
}

/// The OS this binary was built for.
pub const OS: Os = if cfg!(target_os = "macos") {
    Os::Macos
} else if cfg!(target_os = "windows") {
    Os::Windows
} else if cfg!(target_os = "ios") {
    Os::Ios
} else if cfg!(target_os = "android") {
    Os::Android
} else {
    Os::Linux
};

/// The architecture this binary was built for (`X86_64` for anything not aarch64).
pub const ARCH: Arch = if cfg!(target_arch = "aarch64") {
    Arch::Aarch64
} else {
    Arch::X86_64
};

/// The Rust target triple of this build, as release artifacts and the update
/// manifest name it; `"unknown"` off the shipped matrix.
pub const fn target_triple() -> &'static str {
    match (OS, ARCH) {
        (Os::Macos, Arch::Aarch64) => "aarch64-apple-darwin",
        (Os::Macos, Arch::X86_64) => "x86_64-apple-darwin",
        (Os::Linux, Arch::X86_64) => "x86_64-unknown-linux-gnu",
        (Os::Linux, Arch::Aarch64) => "aarch64-unknown-linux-gnu",
        (Os::Windows, Arch::X86_64) => "x86_64-pc-windows-msvc",
        (Os::Windows, Arch::Aarch64) => "aarch64-pc-windows-msvc",
        _ => "unknown",
    }
}

/// One row of the supported-platform matrix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Support {
    /// The OS.
    pub os: Os,
    /// Runs the harness and executes tools (else a client that drives one over the link).
    pub host: bool,
    /// Architectures shipped.
    pub arches: &'static [Arch],
    /// Minimum OS version for the harness.
    pub floor: &'static str,
    /// Extra floors and caveats, one line.
    pub notes: &'static str,
}

/// 58 §0.14: the contract, as data.
pub const SUPPORTED: &[Support] = &[
    Support {
        os: Os::Macos,
        host: true,
        arches: &[Arch::Aarch64, Arch::X86_64],
        floor: "macOS 15",
        notes: "meetings as built (53)",
    },
    Support {
        os: Os::Windows,
        host: true,
        arches: &[Arch::X86_64, Arch::Aarch64],
        floor: "Windows 10 22H2",
        notes: "meetings need Windows 11 (process loopback, AEC)",
    },
    Support {
        os: Os::Linux,
        host: true,
        arches: &[Arch::X86_64, Arch::Aarch64],
        floor: "glibc 2.35",
        notes: "systemd user sessions for the service, PipeWire >= 0.3 for meetings; musl unsupported",
    },
    Support {
        os: Os::Ios,
        host: false,
        arches: &[Arch::Aarch64],
        floor: "iOS 17",
        notes: "client over the link (31)",
    },
    Support {
        os: Os::Android,
        host: false,
        arches: &[Arch::Aarch64],
        floor: "minSdk 26",
        notes: "client over the link; phone-native capture features need API 29",
    },
];

/// This build's row, when it is on the matrix.
pub fn supported() -> Option<&'static Support> {
    SUPPORTED
        .iter()
        .find(|s| s.os == OS && s.arches.contains(&ARCH))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shipped_triple_is_a_matrix_row() {
        assert_eq!(SUPPORTED.iter().filter(|s| s.host).count(), 3);
        assert!(supported().is_some(), "{OS:?}/{ARCH:?} is off the matrix");
        assert_ne!(target_triple(), "unknown");
        assert!(target_triple().contains(if ARCH == Arch::Aarch64 {
            "aarch64"
        } else {
            "x86_64"
        }));
    }
}
