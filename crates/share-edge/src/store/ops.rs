//! Operator-facing store calls (210 §8.5, M4): invites, mesh detail, recover, unattached expiry, the relayed
//! re-count and the engine-key day count.

use super::{Store, ms};

/// Lowercase hex, as [`share_proto::Invite::hash_hex`] prints an invite hash.
pub fn hex_lower(b: &[u8]) -> String {
    use std::fmt::Write as _;
    b.iter().fold(String::with_capacity(b.len() * 2), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

impl Store {
    /// `admin revoke-cert` (§6.3): move a held name's chain to `revocations` with `reason` and forget it; a
    /// `valid` name goes back to `slotted` (it keeps its slot, the holder's next CSR is issued without a token).
    /// False when the name has no chain.
    pub async fn revoke_name_chain(&self, domain: &str, name: &str, reason: &str, now_ms: u64) -> anyhow::Result<bool> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let n = sqlx::query(
            "INSERT INTO revocations (domain, name, chain_pem, reason, next_at_ms, created_ms)
             SELECT domain, name, chain_pem, ?1, ?2, ?2 FROM names
             WHERE domain = ?3 AND name = ?4 AND chain_pem IS NOT NULL",
        )
        .bind(reason)
        .bind(ms(now_ms))
        .bind(domain)
        .bind(name)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        sqlx::query(
            "UPDATE names SET chain_pem = NULL, ari_start_ms = NULL, ari_end_ms = NULL,
               state = CASE state WHEN 'valid' THEN 'slotted' ELSE state END
             WHERE domain = ? AND name = ?",
        )
        .bind(domain)
        .bind(name)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(n > 0)
    }
}

/// An `invites` row.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct InviteRecord {
    /// sha256 of the secret.
    pub hash: Vec<u8>,
    /// Uses left.
    pub uses_left: i64,
    /// Expiry.
    pub expires_ms: i64,
    /// Minted.
    pub created_ms: i64,
    /// Operator note.
    pub note: Option<String>,
}

/// `meshes` columns of `admin mesh`.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MeshDetail {
    /// First seen.
    pub created_ms: i64,
    /// Network first seen from (re-counted when a relayed mesh shows a direct path).
    pub net: String,
    /// Origin ASN.
    pub asn: Option<i64>,
    /// Ban record JSON `{reason, ms, by}`.
    pub banned: Option<String>,
}

/// What [`Store::revoke_invite`] found for a hash prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InviteRevoke {
    /// Revoked: the full hex hash.
    Revoked(String),
    /// No live invite matches.
    NotFound,
    /// More than one matches.
    Ambiguous,
}

impl Store {
    /// Store a minted invite (only its hash).
    pub async fn mint_invite(
        &self,
        hash: &[u8],
        uses: u32,
        expires_ms: u64,
        note: Option<&str>,
        now_ms: u64,
    ) -> anyhow::Result<()> {
        sqlx::query("INSERT INTO invites (hash, uses_left, expires_ms, created_ms, note) VALUES (?, ?, ?, ?, ?)")
            .bind(hash)
            .bind(i64::from(uses))
            .bind(ms(expires_ms))
            .bind(ms(now_ms))
            .bind(note)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Unexpired invites with uses left, newest first.
    pub async fn live_invites(&self, now_ms: u64) -> anyhow::Result<Vec<InviteRecord>> {
        Ok(sqlx::query_as::<_, InviteRecord>(
            "SELECT hash, uses_left, expires_ms, created_ms, note FROM invites
             WHERE uses_left > 0 AND expires_ms > ? ORDER BY created_ms DESC",
        )
        .bind(ms(now_ms))
        .fetch_all(&self.pool)
        .await?)
    }

    /// Revoke the live invite whose hex hash starts with `prefix` (lowercase, ≥ 8 characters checked by the
    /// caller): its uses go to 0.
    pub async fn revoke_invite(&self, prefix: &str, now_ms: u64) -> anyhow::Result<InviteRevoke> {
        let hits: Vec<Vec<u8>> = self
            .live_invites(now_ms)
            .await?
            .into_iter()
            .map(|r| r.hash)
            .filter(|h| hex_lower(h).starts_with(prefix))
            .collect();
        match hits.as_slice() {
            [] => Ok(InviteRevoke::NotFound),
            [h] => {
                sqlx::query("UPDATE invites SET uses_left = 0 WHERE hash = ?")
                    .bind(h)
                    .execute(&self.pool)
                    .await?;
                Ok(InviteRevoke::Revoked(hex_lower(h)))
            }
            _ => Ok(InviteRevoke::Ambiguous),
        }
    }

    /// The `meshes` columns of `admin mesh`.
    pub async fn mesh_detail(&self, mesh: &str) -> anyhow::Result<Option<MeshDetail>> {
        Ok(
            sqlx::query_as::<_, MeshDetail>("SELECT created_ms, net, asn, banned FROM meshes WHERE mesh = ?")
                .bind(mesh)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    /// Engine keys of `mesh` first seen at or after `since_ms`.
    pub async fn engines_first_seen_since(&self, mesh: &str, since_ms: u64) -> anyhow::Result<u64> {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mesh_engines WHERE mesh = ? AND first_ms >= ?")
            .bind(mesh)
            .bind(ms(since_ms))
            .fetch_one(&self.pool)
            .await?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    /// Whether `engine` was ever seen under `mesh`.
    pub async fn engine_known(&self, mesh: &str, engine: &str) -> anyhow::Result<bool> {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mesh_engines WHERE mesh = ? AND engine = ?")
            .bind(mesh)
            .bind(engine)
            .fetch_one(&self.pool)
            .await?;
        Ok(n > 0)
    }

    /// A relayed mesh showed a direct path: its `meshes.net` becomes `net` (§9.4 re-count). True when the row
    /// changed (it was relayed).
    pub async fn rehome_relayed(&self, mesh: &str, net: &str) -> anyhow::Result<bool> {
        let r = sqlx::query("UPDATE meshes SET net = ? WHERE mesh = ? AND net LIKE 'relay:%'")
            .bind(net)
            .bind(mesh)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    /// Held names whose mesh last attached before `before_ms` (or never, created before it).
    pub async fn unattached_names(&self, domain: &str, before_ms: u64) -> anyhow::Result<Vec<String>> {
        Ok(sqlx::query_scalar(
            "SELECT name FROM names WHERE domain = ? AND banned IS NULL
               AND state IN ('allocated','queued','slotted','valid')
               AND COALESCE(last_attach_ms, created_ms) < ? ORDER BY name",
        )
        .bind(domain)
        .bind(ms(before_ms))
        .fetch_all(&self.pool)
        .await?)
    }

    /// Operator recover (§5.7): move `names` (or, when empty, every name `from` holds) to `to`, in one
    /// transaction. `to` gets a `meshes` row (copying `from`'s network and invite flag) when the edge has not
    /// seen it. Transfer offers are cleared; chains and jobs stay with the names (the name key is the family's).
    /// Returns the names moved.
    pub async fn recover(&self, domain: &str, from: &str, to: &str, names: &[String], now_ms: u64) -> anyhow::Result<Vec<String>> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query(
            "INSERT INTO meshes (mesh, created_ms, net, invited)
               SELECT ?, ?, net, invited FROM meshes WHERE mesh = ?
             ON CONFLICT(mesh) DO NOTHING",
        )
        .bind(to)
        .bind(ms(now_ms))
        .bind(from)
        .execute(&mut *tx)
        .await?;
        let held: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM names WHERE domain = ? AND owner = ? AND banned IS NULL
               AND state IN ('allocated','queued','slotted','valid') ORDER BY name",
        )
        .bind(domain)
        .bind(from)
        .fetch_all(&mut *tx)
        .await?;
        let moved: Vec<String> = held
            .into_iter()
            .filter(|n| names.is_empty() || names.contains(n))
            .collect();
        for n in &moved {
            sqlx::query(
                "UPDATE names SET owner = ?, transfer_to = NULL, transfer_until_ms = NULL, last_attach_ms = ?
                 WHERE domain = ? AND name = ?",
            )
            .bind(to)
            .bind(ms(now_ms))
            .bind(domain)
            .bind(n)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE cert_jobs SET mesh = ? WHERE domain = ? AND name = ? AND state IN ('queued','ordering')",
            )
            .bind(to)
            .bind(domain)
            .bind(n)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(moved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DbJournal;
    use crate::store::NewName;

    #[tokio::test]
    async fn invites_recover_and_unattached() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("e.sqlite"), DbJournal::Wal).await.unwrap();
        let t0 = 1_000_000u64;
        s.mint_invite(&[0xab; 32], 1, t0 + 10, Some("ops"), t0).await.unwrap();
        s.mint_invite(&[0xac; 32], 2, t0 + 10, None, t0).await.unwrap();
        assert_eq!(s.live_invites(t0).await.unwrap().len(), 2);
        assert_eq!(s.live_invites(t0 + 10).await.unwrap().len(), 0, "expired");
        assert_eq!(s.revoke_invite("a", t0).await.unwrap(), InviteRevoke::Ambiguous);
        assert_eq!(
            s.revoke_invite("abab", t0).await.unwrap(),
            InviteRevoke::Revoked(hex_lower(&[0xab; 32]))
        );
        assert!(!s.consume_invite(&[0xab; 32], t0).await.unwrap(), "revoked invite is spent");
        assert!(s.consume_invite(&[0xac; 32], t0).await.unwrap());
        assert_eq!(s.revoke_invite("abab", t0).await.unwrap(), InviteRevoke::NotFound);

        s.attach("m1", "e1", "relay:https://r/", false, t0).await.unwrap();
        for name in ["one", "two"] {
            s.claim(
                NewName { domain: "tun.zip", name, kind: "reserved", owner: "m1", now_ms: ms(t0), net: "n" },
                3,
            )
            .await
            .unwrap();
        }
        assert!(s.rehome_relayed("m1", "192.0.2.9").await.unwrap());
        assert!(!s.rehome_relayed("m1", "192.0.2.10").await.unwrap(), "only a relayed row moves");
        assert_eq!(s.mesh_detail("m1").await.unwrap().unwrap().net, "192.0.2.9");
        assert_eq!(s.engines_first_seen_since("m1", t0).await.unwrap(), 1);
        assert_eq!(s.engines_first_seen_since("m1", t0 + 1).await.unwrap(), 0);

        let moved = s.recover("tun.zip", "m1", "m2", &["two".into()], t0 + 5).await.unwrap();
        assert_eq!(moved, vec!["two".to_string()]);
        assert_eq!(s.name("tun.zip", "two").await.unwrap().unwrap().owner.as_deref(), Some("m2"));
        assert!(s.mesh("m2").await.unwrap().is_some(), "to-mesh row created");

        assert_eq!(s.unattached_names("tun.zip", t0 + 1).await.unwrap(), vec!["one".to_string()]);
        assert!(s.unattached_names("tun.zip", t0).await.unwrap().is_empty());
    }

    /// FIX4-EDGE #2, #7, #18, #20, #22.
    #[tokio::test]
    async fn bans_unhold_revokes_stay_known_pause_persists_invites_spend_with_attach() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("e.sqlite");
        let s = Store::open(&path, DbJournal::Wal).await.unwrap();
        let t0 = 1_000_000u64;
        s.attach("m1", "e1", "192.0.2.1", false, t0).await.unwrap();
        for domain in ["tun.zip", "other.zip"] {
            let n = NewName { domain, name: "sky", kind: "reserved", owner: "m1", now_ms: ms(t0), net: "n" };
            assert_eq!(s.claim(n, 3).await.unwrap(), crate::store::Claimed::Granted);
        }
        // #2: a banned name is not held, under its domain only (#22).
        s.ban("tun.zip", "name", "sky", "abuse", t0).await.unwrap();
        assert!(s.held_names("tun.zip", "m1").await.unwrap().is_empty());
        assert!(!s.name("tun.zip", "sky").await.unwrap().unwrap().held());
        assert_eq!(s.held_names("other.zip", "m1").await.unwrap().len(), 1, "other domain untouched");
        assert!(s.unban("tun.zip", "name", "sky").await.unwrap());
        assert_eq!(s.held_names("tun.zip", "m1").await.unwrap().len(), 1);

        // #7: only engines the mesh has seen, at most `cap` revoked.
        s.attach("m1", "e2", "192.0.2.1", false, t0).await.unwrap();
        let ids = |v: &[&str]| v.iter().map(|x| (*x).to_string()).collect::<Vec<_>>();
        assert_eq!(s.revoke("m1", &ids(&["e2", "made-up"]), 4, t0).await.unwrap(), Some(ids(&["e2"])));
        assert_eq!(s.engines("m1").await.unwrap().len(), 2, "no row for an unknown engine");
        assert_eq!(s.revoke("m1", &ids(&["e1"]), 1, t0).await.unwrap(), None, "over the cap");
        assert!(s.revoked("m1", "e1").await.unwrap().is_none());

        // #20: the invite is spent by the attach that uses it, and only once.
        s.mint_invite(&[0xab; 32], 1, t0 + 10, None, t0).await.unwrap();
        assert!(s.invite_live(&[0xab; 32], t0).await.unwrap());
        assert!(s.attach_invited("m2", "e3", "n", &[0xab; 32], t0).await.unwrap());
        assert!(!s.attach_invited("m3", "e4", "n", &[0xab; 32], t0).await.unwrap());
        assert!(s.mesh("m3").await.unwrap().is_none(), "a refused attach writes nothing");
        assert_eq!(s.mesh("m2").await.unwrap().unwrap().invited, 1);

        // #18: pause survives a reopen.
        assert!(!s.paused().await.unwrap());
        s.set_paused(true).await.unwrap();
        s.close().await;
        let s = Store::open(&path, DbJournal::Wal).await.unwrap();
        assert!(s.paused().await.unwrap());
    }
}
