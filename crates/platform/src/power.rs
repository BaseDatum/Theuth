//! The machine's physical budget (126 §8.2): mains or battery, thermal state,
//! 1-minute load, free disk, memory pressure. Every probe answers `Unknown` /
//! `None` where the platform has no cheap source; the caller decides what
//! unknown means (the training preconditions wait on an unknown power or
//! idle state and let the others pass).
//!
//! Sources:
//! - power — macOS: `pmset -g batt` parsed (`'AC Power'` / `'Battery Power'`);
//!   no IOKit binding is in the tree without a new dependency, and `pmset`
//!   ships with every macOS. Linux: `/sys/class/power_supply/*/online` of a
//!   `Mains` supply. Windows: `Unknown`.
//! - thermal — macOS: `NSProcessInfo.thermalState`. Others: `Unknown`.
//! - load — `getloadavg(3)` on Unix; `None` elsewhere.
//! - free disk — `statvfs(3)` on Unix; `None` elsewhere.
//! - memory — macOS: `sysctl kern.memorystatus_vm_pressure_level` (1 normal,
//!   2 warn, 4 critical). Linux: `/proc/pressure/memory` (`some avg10`).
//!   Windows: `Unknown`.

use std::path::Path;

/// Mains or battery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Power {
    /// Plugged in.
    Ac,
    /// Running on the battery.
    Battery,
    /// No probe on this platform, or the probe failed.
    Unknown,
}

/// `NSProcessInfoThermalState` and its moral equivalents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Thermal {
    /// Normal.
    Nominal,
    /// Slightly elevated; the OS may already throttle a little.
    Fair,
    /// Throttled.
    Serious,
    /// Heavily throttled; the machine is close to shutting down.
    Critical,
    /// No probe on this platform.
    Unknown,
}

/// Memory pressure, as the kernel reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryPressure {
    /// Normal.
    Normal,
    /// Warning / some stall.
    Warn,
    /// Critical.
    Critical,
    /// No probe on this platform.
    Unknown,
}

/// Mains or battery right now.
pub fn power() -> Power {
    imp::power()
}

/// The thermal state right now.
pub fn thermal() -> Thermal {
    imp::thermal()
}

/// The 1-minute load average.
pub fn load1() -> Option<f64> {
    imp::load1()
}

/// Free bytes on the filesystem holding `path` (for the unprivileged user).
pub fn free_disk_bytes(path: &Path) -> Option<u64> {
    imp::free_disk_bytes(path)
}

/// Memory pressure right now.
pub fn memory_pressure() -> MemoryPressure {
    imp::memory_pressure()
}

/// `pmset -g batt` → [`Power`]: the first line names the source.
pub fn parse_pmset(out: &str) -> Power {
    if out.contains("'AC Power'") {
        Power::Ac
    } else if out.contains("'Battery Power'") {
        Power::Battery
    } else {
        Power::Unknown
    }
}

#[cfg(unix)]
mod unix {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;
    use std::path::Path;

    pub fn load1() -> Option<f64> {
        let mut avg = [0f64; 3];
        // SAFETY: getloadavg writes at most `3` doubles into a 3-double array.
        let n = unsafe { libc::getloadavg(avg.as_mut_ptr(), 3) };
        (n >= 1).then_some(avg[0])
    }

    pub fn free_disk_bytes(path: &Path) -> Option<u64> {
        let c = CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: a zeroed statvfs is a valid out-parameter; `c` is NUL-terminated.
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::statvfs(c.as_ptr(), &mut st) };
        // `f_bavail` / `f_frsize` are u32 or u64 by platform; widen without a `From` clippy would call useless.
        (rc == 0).then(|| st.f_bavail as u64 * st.f_frsize as u64)
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{MemoryPressure, Power, Thermal};
    use objc2_foundation::{NSProcessInfo, NSProcessInfoThermalState};

    pub use super::unix::{free_disk_bytes, load1};

    pub fn power() -> Power {
        match std::process::Command::new("pmset")
            .args(["-g", "batt"])
            .output()
        {
            Ok(o) if o.status.success() => super::parse_pmset(&String::from_utf8_lossy(&o.stdout)),
            _ => Power::Unknown,
        }
    }

    pub fn thermal() -> Thermal {
        match NSProcessInfo::processInfo().thermalState() {
            NSProcessInfoThermalState::Nominal => Thermal::Nominal,
            NSProcessInfoThermalState::Fair => Thermal::Fair,
            NSProcessInfoThermalState::Serious => Thermal::Serious,
            NSProcessInfoThermalState::Critical => Thermal::Critical,
            _ => Thermal::Unknown,
        }
    }

    pub fn memory_pressure() -> MemoryPressure {
        let mut level: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>();
        // SAFETY: the name is NUL-terminated; `level`/`len` are valid out-parameters of the sizes given.
        let rc = unsafe {
            libc::sysctlbyname(
                c"kern.memorystatus_vm_pressure_level".as_ptr(),
                (&mut level as *mut libc::c_int).cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc != 0 {
            return MemoryPressure::Unknown;
        }
        match level {
            1 => MemoryPressure::Normal,
            2 => MemoryPressure::Warn,
            4 => MemoryPressure::Critical,
            _ => MemoryPressure::Unknown,
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{MemoryPressure, Power, Thermal};

    pub use super::unix::{free_disk_bytes, load1};

    pub fn power() -> Power {
        let Ok(dir) = std::fs::read_dir("/sys/class/power_supply") else {
            return Power::Unknown;
        };
        let mut seen = false;
        for e in dir.flatten() {
            let p = e.path();
            let mains = std::fs::read_to_string(p.join("type"))
                .map(|t| t.trim() == "Mains")
                .unwrap_or(false);
            if !mains {
                continue;
            }
            seen = true;
            if std::fs::read_to_string(p.join("online")).is_ok_and(|o| o.trim() == "1") {
                return Power::Ac;
            }
        }
        if seen { Power::Battery } else { Power::Unknown }
    }

    pub fn thermal() -> Thermal {
        Thermal::Unknown
    }

    pub fn memory_pressure() -> MemoryPressure {
        let Ok(s) = std::fs::read_to_string("/proc/pressure/memory") else {
            return MemoryPressure::Unknown;
        };
        // `some avg10=0.00 avg60=0.00 avg300=0.00 total=0`
        let some = s
            .lines()
            .find(|l| l.starts_with("some"))
            .and_then(|l| {
                l.split_whitespace()
                    .find_map(|kv| kv.strip_prefix("avg10="))
            })
            .and_then(|v| v.parse::<f64>().ok());
        match some {
            Some(v) if v < 5.0 => MemoryPressure::Normal,
            Some(v) if v < 25.0 => MemoryPressure::Warn,
            Some(_) => MemoryPressure::Critical,
            None => MemoryPressure::Unknown,
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod imp {
    use super::{MemoryPressure, Power, Thermal};
    use std::path::Path;

    pub fn power() -> Power {
        Power::Unknown
    }
    pub fn thermal() -> Thermal {
        Thermal::Unknown
    }
    pub fn load1() -> Option<f64> {
        None
    }
    pub fn free_disk_bytes(_: &Path) -> Option<u64> {
        None
    }
    pub fn memory_pressure() -> MemoryPressure {
        MemoryPressure::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pmset_first_line_names_the_source() {
        assert_eq!(
            parse_pmset(
                "Now drawing from 'AC Power'\n -InternalBattery-0 (id=123)\t100%; charged; 0:00 remaining present: true\n"
            ),
            Power::Ac
        );
        assert_eq!(
            parse_pmset(
                "Now drawing from 'Battery Power'\n -InternalBattery-0\t83%; discharging; 4:10 remaining present: true\n"
            ),
            Power::Battery
        );
        assert_eq!(parse_pmset(""), Power::Unknown);
    }

    #[cfg(unix)]
    #[test]
    fn unix_probes_answer() {
        assert!(load1().is_some_and(|l| l >= 0.0));
        assert!(free_disk_bytes(Path::new("/")).is_some_and(|b| b > 0));
    }
}
