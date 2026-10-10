//! Cases and the stable three-way split (159 W-g; dreaming's two-way split is the special case
//! `Shares { train: 66, val: 34, holdout: 0 }` with seed `"136"`).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// One case of a set: JSONL `{id, input, ideal?, check?, tags}` for a `file` source, or a
/// dreaming case (`context` → `input`, `ideal` + `user_words` → `references`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Case {
    /// Stable id; the split hashes it.
    pub id: String,
    /// The user message the artefact answers.
    pub input: String,
    /// The reference answer for the judge or an exact checker; never shown to the session.
    pub ideal: Option<String>,
    /// Extra references for the judge (dreaming's `user_words`).
    pub references: Vec<String>,
    /// How the answer is scored; `None` = the judge.
    pub check: Option<Check>,
    /// Free tags (a case class, a cluster).
    pub tags: Vec<String>,
    /// Synthetic cases are never val or holdout (dreaming's rule).
    pub synthetic: bool,
}

/// How a case's answer becomes a 0–1 score without a judge.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Check {
    /// `answer.trim() == expected`.
    Exact(String),
    /// The regex matches the answer.
    Regex(String),
    /// The answer contains the text.
    Contains(String),
    /// A custody command that reads the answer on stdin and prints `METRIC score=`.
    Command(String),
}

/// Which split a case is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Split {
    /// Shown to the session as feedback.
    Train,
    /// Decides keeps.
    Val,
    /// Never decides; the overfit check and finalize read it.
    Holdout,
}

impl Split {
    /// serde's word.
    pub fn word(self) -> &'static str {
        match self {
            Self::Train => "train",
            Self::Val => "val",
            Self::Holdout => "holdout",
        }
    }
}

/// Percent shares of the three splits; they must sum to 100.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shares {
    /// Train percent.
    pub train: u8,
    /// Val percent.
    pub val: u8,
    /// Holdout percent.
    pub holdout: u8,
}

impl Default for Shares {
    /// 159 W-g: 40 train / 40 val / 20 holdout.
    fn default() -> Self {
        Self {
            train: 40,
            val: 40,
            holdout: 20,
        }
    }
}

/// The stable split: bucket = `sha256(seed ‖ id)` first eight bytes mod 100; `< val` → val,
/// `< val + train` → train, else holdout. Val comes first so dreaming's two-way split (seed
/// `"136"`, `bucket < 34 → val`) is this function with `Shares { train: 66, val: 34, holdout: 0 }`,
/// bit for bit (159 W2-a). A synthetic case is always train.
pub fn split(seed: &str, id: &str, shares: Shares, synthetic: bool) -> Split {
    if synthetic {
        return Split::Train;
    }
    let mut h = Sha256::new();
    h.update(seed.as_bytes());
    h.update(id.as_bytes());
    let d = h.finalize();
    let bucket = u64::from_be_bytes(d[..8].try_into().expect("8 bytes")) % 100;
    let bucket = bucket as u8;
    if bucket < shares.val {
        Split::Val
    } else if bucket < shares.val.saturating_add(shares.train) {
        Split::Train
    } else {
        Split::Holdout
    }
}

/// sha256 hex of an artefact's text: the key replay rows are stored under.
pub fn candidate_hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_is_stable_and_near_the_shares() {
        let shares = Shares::default();
        let mut counts = [0u32; 3];
        for i in 0..10_000 {
            let s = split("slug", &format!("case-{i}"), shares, false);
            assert_eq!(s, split("slug", &format!("case-{i}"), shares, false));
            counts[match s {
                Split::Train => 0,
                Split::Val => 1,
                Split::Holdout => 2,
            }] += 1;
        }
        assert!((3_700..4_300).contains(&counts[0]), "{counts:?}");
        assert!((3_700..4_300).contains(&counts[1]), "{counts:?}");
        assert!((1_700..2_300).contains(&counts[2]), "{counts:?}");
        assert_eq!(split("slug", "any", shares, true), Split::Train);
        // Seeds matter: over many ids, two seeds disagree on some split.
        let disagree = (0..200).any(|i| {
            split("a", &format!("c{i}"), shares, false)
                != split("b", &format!("c{i}"), shares, false)
        });
        assert!(disagree);
    }

    #[test]
    fn hash_is_sha256_hex() {
        assert_eq!(candidate_hash("").len(), 64);
        assert_ne!(candidate_hash("a"), candidate_hash("b"));
    }
}
