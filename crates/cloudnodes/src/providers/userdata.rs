//! The VM's first-boot script (design doc 224 §9): `deploy/computer/vm-cloud-init.sh` with its
//! placeholders filled. Every value is charset-checked, so nothing can break out of the script's
//! single quotes.

use base64::Engine as _;

use super::{CloudError, Result, check_name};

const TEMPLATE: &str = include_str!("../../../../deploy/computer/vm-cloud-init.sh");
const INSTALL_SH: &[u8] = include_bytes!("../../../../deploy/computer/theuth-install.sh");
const PUBKEY: &str = include_str!("../../../update/pubkey.txt");

/// `1.` + base32 (a key-less pairing ticket).
pub fn is_ticket(t: &str) -> bool {
    t.strip_prefix("1.").is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .bytes()
                .all(|b| b.is_ascii_alphabetic() || (b'2'..=b'7').contains(&b))
    })
}

fn safe_chars(s: &str) -> bool {
    !s.is_empty()
        && s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/_.:-".contains(&b))
}

/// The user data for node `name`; `data_dev` the volume's block device, `ticket` empty for none,
/// `desktop` the watchable desktop (Xvnc + websockify on 127.0.0.1, reached over the mesh).
pub fn render(name: &str, data_dev: &str, ticket: &str, manifest: &str, desktop: bool) -> Result<String> {
    check_name(name)?;
    // A mesh ticket, or a phone's one-time link claim (`L1.…`, 224 §16.12); both `[0-9A-Za-z.]`.
    if !ticket.is_empty() && !is_ticket(ticket) && !link::claim::looks_like(ticket) {
        return Err(CloudError("not a pairing ticket".into()));
    }
    let manifest_ok = manifest
        .strip_prefix("https://")
        .is_some_and(safe_chars);
    if !safe_chars(data_dev) || !manifest_ok {
        return Err(CloudError("bad device path or manifest URL".into()));
    }
    let install = base64::engine::general_purpose::STANDARD.encode(INSTALL_SH);
    Ok(TEMPLATE
        .replace("__NAME__", name)
        .replace("__DATA_DEV__", data_dev)
        .replace("__TICKET__", ticket)
        .replace("__MANIFEST__", manifest)
        .replace("__PUBKEY__", PUBKEY.trim())
        .replace("__INSTALL_SH__", &install)
        .replace("__DESKTOP__", if desktop { "1" } else { "0" }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::catalog::MANIFEST;

    const DEV: &str = "/dev/disk/by-id/scsi-0DO_Volume_theuth-lab-data";

    #[test]
    fn fills_every_placeholder() {
        let t = "1.ABCDEFGH234567";
        let s = render("lab", DEV, t, MANIFEST, true).unwrap();
        for p in ["__NAME__", "__DATA_DEV__", "__TICKET__", "__MANIFEST__", "__PUBKEY__", "__INSTALL_SH__", "__DESKTOP__"] {
            assert!(!s.contains(p), "{p}");
        }
        assert_eq!(s.matches(t).count(), 1);
        assert!(s.contains("name='lab'") && s.contains(&format!("dev='{DEV}'")));
        assert!(s.contains(&format!("THEUTH_MANIFEST={MANIFEST}")));
        // Only the header comment says "no Docker"; no command uses it.
        assert!(!s.lines().filter(|l| !l.trim_start().starts_with('#')).any(|l| l.to_lowercase().contains("docker")));
        assert!(s.contains("/etc/sudoers.d/90-theuth-engine") && s.contains("NOPASSWD:ALL"));
        assert!(s.contains(PUBKEY.trim()));

        // The install script, byte for byte.
        let b64 = s
            .lines()
            .find_map(|l| l.strip_prefix("echo '")?.strip_suffix("' | base64 -d >/usr/local/lib/theuth-install.sh"))
            .unwrap();
        let decoded = base64::engine::general_purpose::STANDARD.decode(b64).unwrap();
        assert_eq!(decoded, INSTALL_SH);

        assert!(s.contains("desktop='1'") && s.contains("theuth-xvnc.service") && s.contains("websockify 127.0.0.1:6080"));

        let empty = render("lab", DEV, "", MANIFEST, false).unwrap();
        assert!(empty.contains("ticket=''") && empty.contains("desktop='0'"));
    }

    #[test]
    fn rejects_injection() {
        assert!(render("lab", DEV, "1.ABC'; rm -rf /", MANIFEST, true).is_err());
        assert!(render("lab", DEV, "2.ABC", MANIFEST, true).is_err());
        assert!(render("lab", DEV, "1.abc189", MANIFEST, true).is_err());
        assert!(render("lab", "/dev/x'$(id)", "", MANIFEST, true).is_err());
        assert!(render("lab", DEV, "", "http://insecure/latest.json", true).is_err());
        assert!(render("lab", DEV, "", "https://x/a b", true).is_err());
        assert!(render("Lab'", DEV, "", MANIFEST, true).is_err());
    }
}
