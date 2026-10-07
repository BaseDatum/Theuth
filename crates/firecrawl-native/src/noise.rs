//! Noise safety valves (design doc 122 D8): whether a strip-list match may be
//! removed and whether a node is noise for candidate scoring. Contract stub —
//! every match may be stripped and nothing is noise until W2 lands the valves.

use kuchikiki::NodeRef;

/// Whether a strip-list match may be removed (text valve, `<form>` rule,
/// structural ids, consent prefixes).
///
/// Contract stub (122 §7.1): `true`.
#[must_use]
pub fn may_strip(_node: &NodeRef) -> bool {
    true
}

/// Whether `node` is noise for candidate scoring (tag, role, class token, id, consent prefix).
///
/// Contract stub (122 §7.1): `false`.
#[must_use]
pub fn is_noise(_node: &NodeRef) -> bool {
    false
}
