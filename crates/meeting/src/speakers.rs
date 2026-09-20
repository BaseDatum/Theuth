//! Speaker profiles (04 §4, 53 decision 6): `<data_dir>/meetings/speakers/<uuid>.json`,
//! one file per enrolled person — name, optional knowledge entity, the running
//! mean of their voice embeddings. Matching is cosine against that mean;
//! ≥ [`MATCH`] is proposed, never applied without the user.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use crate::ProfileId;

/// Directory under `<data_dir>/meetings/`.
pub const DIR: &str = "speakers";
/// Cosine at or above which a profile is proposed for a cluster (04 §0.3).
pub const MATCH: f32 = 0.6;

/// What the wire sees (`meeting_speakers`): the embedding stays in the file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Profile {
    /// File name.
    pub id: ProfileId,
    /// Display name.
    pub name: String,
    /// Knowledge entity id, if linked.
    pub entity: Option<String>,
    /// Enrollments folded into the mean.
    pub samples: u32,
    /// RFC 3339.
    pub created_at: String,
    /// RFC 3339.
    pub updated_at: String,
}

/// The file: the wire fields plus the mean embedding.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Stored {
    #[serde(flatten)]
    profile: Profile,
    embedding: Vec<f32>,
}

/// The enrolled profiles, loaded from disk; every change is written through.
pub struct Profiles {
    dir: PathBuf,
    list: Vec<Stored>,
}

/// Cosine similarity; 0 for a zero vector or mismatched lengths.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0f32, 0f32, 0f32);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

impl Profiles {
    /// Read every `*.json` under `dir` (created if missing); unreadable files are skipped.
    pub fn load(dir: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let mut list = Vec::new();
        for entry in std::fs::read_dir(dir)?.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            match std::fs::read(&path).map_err(anyhow::Error::from).and_then(|b| {
                serde_json::from_slice::<Stored>(&b).map_err(anyhow::Error::from)
            }) {
                Ok(s) => list.push(s),
                Err(e) => tracing::warn!("speaker profile {}: {e:#}", path.display()),
            }
        }
        list.sort_by(|a, b| a.profile.name.cmp(&b.profile.name));
        Ok(Self {
            dir: dir.to_owned(),
            list,
        })
    }

    /// The profiles, by name.
    pub fn list(&self) -> Vec<Profile> {
        self.list.iter().map(|s| s.profile.clone()).collect()
    }

    /// Best cosine ≥ [`MATCH`]: `(id, name, score)`.
    pub fn best_match(&self, emb: &[f32]) -> Option<(ProfileId, String, f32)> {
        self.list
            .iter()
            .map(|s| (s, cosine(&s.embedding, emb)))
            .filter(|(_, c)| *c >= MATCH)
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(s, c)| (s.profile.id, s.profile.name.clone(), c))
    }

    /// New profile, or the running mean into the one already named `name`
    /// (case-insensitive). `entity` replaces the stored one when given.
    pub fn enroll(
        &mut self,
        name: &str,
        entity: Option<String>,
        emb: &[f32],
    ) -> Result<ProfileId, String> {
        if emb.is_empty() {
            return Err("empty embedding".into());
        }
        let now = chrono::Local::now().to_rfc3339();
        let id = match self
            .list
            .iter_mut()
            .find(|s| s.profile.name.eq_ignore_ascii_case(name))
        {
            Some(s) => {
                if s.embedding.len() != emb.len() {
                    return Err("embedding size differs from the profile's".into());
                }
                let n = s.profile.samples as f32;
                for (m, x) in s.embedding.iter_mut().zip(emb) {
                    *m = (*m * n + x) / (n + 1.0);
                }
                s.profile.samples += 1;
                s.profile.updated_at = now;
                if entity.is_some() {
                    s.profile.entity = entity;
                }
                s.profile.id
            }
            None => {
                let id = ProfileId::new();
                self.list.push(Stored {
                    profile: Profile {
                        id,
                        name: name.to_owned(),
                        entity,
                        samples: 1,
                        created_at: now.clone(),
                        updated_at: now,
                    },
                    embedding: emb.to_vec(),
                });
                id
            }
        };
        let s = self.list.iter().find(|s| s.profile.id == id).expect("just placed");
        let path = self.path(id);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(s).map_err(|e| e.to_string())?)
            .and_then(|()| std::fs::rename(&tmp, &path))
            .map_err(|e| format!("write {}: {e}", path.display()))?;
        Ok(id)
    }

    /// Remove the file; `Err` when unknown.
    pub fn delete(&mut self, id: ProfileId) -> Result<(), String> {
        let i = self
            .list
            .iter()
            .position(|s| s.profile.id == id)
            .ok_or_else(|| format!("no speaker profile {id}"))?;
        self.list.remove(i);
        std::fs::remove_file(self.path(id)).map_err(|e| format!("remove profile {id}: {e}"))
    }

    fn path(&self, id: ProfileId) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_threshold_and_running_mean_survive_reload() {
        let tmp = tempfile::tempdir().unwrap();
        let mut p = Profiles::load(tmp.path()).unwrap();
        assert!(p.best_match(&[1.0, 0.0]).is_none(), "nothing enrolled");
        let dana = p.enroll("Dana", None, &[1.0, 0.0]).unwrap();
        let john = p.enroll("John", Some("people/john".into()), &[0.0, 1.0]).unwrap();
        // (1, 0.9): cosine 0.743 to Dana, 0.669 to John → Dana; (0.17, 0.98) → John; (-1,-1) → nobody.
        let (id, name, score) = p.best_match(&[1.0, 0.9]).unwrap();
        assert_eq!((id, name.as_str()), (dana, "Dana"));
        assert!((score - 0.7433).abs() < 1e-3, "{score}");
        assert!(p.best_match(&[0.17, 0.98]).is_some_and(|m| m.0 == john));
        assert!(p.best_match(&[-1.0, -1.0]).is_none());
        // Running mean: (1,0) then (0,1) → (0.5, 0.5), samples 2, same id, case-insensitive.
        assert_eq!(p.enroll("dana", None, &[0.0, 1.0]).unwrap(), dana);
        let reloaded = Profiles::load(tmp.path()).unwrap();
        let d = reloaded.list.iter().find(|s| s.profile.id == dana).unwrap();
        assert_eq!(d.embedding, vec![0.5, 0.5]);
        assert_eq!(d.profile.samples, 2);
        assert_eq!(reloaded.list().len(), 2);
        assert!(p.enroll("john", None, &[1.0]).is_err(), "size mismatch into a profile is refused");
        let mut p = reloaded;
        p.delete(john).unwrap();
        assert!(p.delete(john).is_err());
        assert_eq!(Profiles::load(tmp.path()).unwrap().list().len(), 1);
    }
}
