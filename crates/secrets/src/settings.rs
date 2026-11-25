//! 221 D20: every external-secret gate is a setting with a safe default. Per connection under
//! `secrets.connections.<conn>`, harness-wide under `secrets.external`. The four ask directions
//! are not here: they are 219 entries in `tools.always_ask` (`tools::secrets::ask_keys`).
//!
//! [`set`] changes one key and says whether the change loosens it: the RPC that writes a key
//! applies a loosening from an owner press directly and raises the human-only card for any other
//! caller (219 §2.4); tightening is never gated.

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Which items `create` / `update` / `delete` may touch (D20 `write_scope`).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum WriteScope {
    /// The personal vault only.
    #[default]
    Personal,
    /// Organisation collections too.
    PersonalAndOrg,
}

/// The per-connection D20 settings (`secrets.connections.<conn>.…`), each at its safe default.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct ConnSettings {
    /// A Send's card is answerable only by the owner in person.
    pub send_owner_only: bool,
    /// `permanent: true` deletes skip the recovery window / trash.
    pub allow_permanent_delete: bool,
    /// AWS recovery window for a delete, 7..=30 days.
    pub delete_recovery_days: u8,
    /// Seal the Bitwarden user key in `meta` so a restart needs no master password.
    pub remember_unlock: bool,
    /// Lock after this long without use; `0` = only on quit.
    pub lock_after_idle_secs: u64,
    /// Lock on sleep or screen lock.
    pub lock_on_sleep: bool,
    /// A logged-out connection raises the login card for the agent's call instead of failing.
    pub agent_may_request_login: bool,
    /// `bw/…#password:-N` reaches the login's password history.
    pub expose_password_history: bool,
    /// `#totp-seed` expands the seed, not only the current code.
    pub expose_totp_seed: bool,
    /// Whether writes may target organisation collections.
    pub write_scope: WriteScope,
    /// External references in `mcp/*.json` expand at connect time (only while `read` is not
    /// always ask).
    pub mcp_config_refs: bool,
    /// List and revoke Sends theuth did not create.
    pub manage_all_sends: bool,
    /// The Send receipt carries the link itself (D23).
    pub send_link_to_agent: bool,
    /// The owner's Reveal action on this connection's values (D25).
    pub allow_reveal: bool,
}

impl Default for ConnSettings {
    fn default() -> Self {
        Self {
            send_owner_only: true,
            allow_permanent_delete: false,
            delete_recovery_days: 30,
            remember_unlock: false,
            lock_after_idle_secs: 900,
            lock_on_sleep: true,
            agent_may_request_login: false,
            expose_password_history: false,
            expose_totp_seed: false,
            write_scope: WriteScope::Personal,
            mcp_config_refs: false,
            manage_all_sends: false,
            send_link_to_agent: false,
            allow_reveal: true,
        }
    }
}

/// The harness-wide D20 settings and the backend-call bound (`secrets.external.…`).
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct ExternalSettings {
    /// Every backend call runs under this bound (221 §4).
    pub timeout_secs: u64,
    /// Largest file a Send uploads; the server's own limit still applies.
    pub send_max_file_mb: u64,
    /// `copy_send_link` from a non-press caller raises a human-only card.
    pub copy_link_needs_press: bool,
}

impl Default for ExternalSettings {
    fn default() -> Self {
        Self {
            timeout_secs: 20,
            send_max_file_mb: 100,
            copy_link_needs_press: true,
        }
    }
}

/// One `secrets.connections.<conn>` entry: its backend, the D20 settings, and the backend's own
/// keys (profile, region, server URLs, …), which the backend crate reads.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConnectionConfig {
    /// `aws` | `bw`.
    pub backend: String,
    /// The D20 settings.
    #[serde(flatten)]
    pub settings: ConnSettings,
    /// Everything else, for the backend.
    #[serde(flatten)]
    pub other: BTreeMap<String, serde_yaml::Value>,
}

/// Whether moving `key` from `old` to `new` loosens a gate (D20). `Err` = not a D20 key.
pub fn loosens(
    key: &str,
    old: &serde_json::Value,
    new: &serde_json::Value,
) -> Result<bool, String> {
    let flag = |v: &serde_json::Value| v.as_bool().unwrap_or(false);
    let num = |v: &serde_json::Value| v.as_u64().unwrap_or(0);
    Ok(match key {
        // On is looser.
        "allow_permanent_delete"
        | "remember_unlock"
        | "agent_may_request_login"
        | "expose_password_history"
        | "expose_totp_seed"
        | "mcp_config_refs"
        | "manage_all_sends"
        | "send_link_to_agent"
        | "allow_reveal" => !flag(old) && flag(new),
        // Off is looser.
        "send_owner_only" | "lock_on_sleep" | "copy_link_needs_press" => flag(old) && !flag(new),
        // Longer is looser; 0 = never.
        "lock_after_idle_secs" => {
            let idle = |v| match num(v) {
                0 => u64::MAX,
                n => n,
            };
            idle(new) > idle(old)
        }
        // A shorter recovery window is looser.
        "delete_recovery_days" => num(new) < num(old),
        "send_max_file_mb" => num(new) > num(old),
        "write_scope" => {
            old.as_str() == Some("personal") && new.as_str() == Some("personal_and_org")
        }
        // A bound, not a gate.
        "timeout_secs" => false,
        _ => return Err(format!("{key} is not a secrets setting")),
    })
}

/// `current` with `key` set to `value`, and whether that loosens it ([`loosens`]). `Err` = no such
/// key, a value of the wrong type, or out of range.
pub fn set<T: Serialize + DeserializeOwned + Validate>(
    current: &T,
    key: &str,
    value: serde_json::Value,
) -> Result<(T, bool), String> {
    let mut map = match serde_json::to_value(current) {
        Ok(serde_json::Value::Object(m)) => m,
        _ => return Err("settings are not an object".to_owned()),
    };
    let Some(old) = map.get(key).cloned() else {
        return Err(format!("{key} is not a secrets setting"));
    };
    map.insert(key.to_owned(), value);
    let next: T = serde_json::from_value(serde_json::Value::Object(map))
        .map_err(|e| format!("{key}: {e}"))?;
    next.validate()?;
    let new = serde_json::to_value(&next)
        .ok()
        .and_then(|v| v.get(key).cloned())
        .unwrap_or_default();
    Ok((next, loosens(key, &old, &new)?))
}

/// Range checks a [`set`] runs before it returns.
pub trait Validate {
    /// `Err` names the key and its range.
    fn validate(&self) -> Result<(), String>;
}

impl Validate for ConnSettings {
    fn validate(&self) -> Result<(), String> {
        if !(7..=30).contains(&self.delete_recovery_days) {
            return Err("delete_recovery_days: 7 to 30".to_owned());
        }
        Ok(())
    }
}

impl Validate for ExternalSettings {
    fn validate(&self) -> Result<(), String> {
        if self.timeout_secs == 0 {
            return Err("timeout_secs: at least 1".to_owned());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Per key: a non-default value, written as YAML.
    const CONN: &[(&str, &str)] = &[
        ("send_owner_only", "false"),
        ("allow_permanent_delete", "true"),
        ("delete_recovery_days", "7"),
        ("remember_unlock", "true"),
        ("lock_after_idle_secs", "0"),
        ("lock_on_sleep", "false"),
        ("agent_may_request_login", "true"),
        ("expose_password_history", "true"),
        ("expose_totp_seed", "true"),
        ("write_scope", "personal_and_org"),
        ("mcp_config_refs", "true"),
        ("manage_all_sends", "true"),
        ("send_link_to_agent", "true"),
        ("allow_reveal", "false"),
    ];
    const HARNESS: &[(&str, &str)] = &[
        ("timeout_secs", "45"),
        ("send_max_file_mb", "500"),
        ("copy_link_needs_press", "false"),
    ];

    fn check_keys<T>(table: &[(&str, &str)])
    where
        T: Default + PartialEq + std::fmt::Debug + Serialize + DeserializeOwned + Validate,
    {
        let defaults = serde_json::to_value(T::default()).unwrap();
        let mut keys: Vec<&str> = defaults
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        let mut listed: Vec<&str> = table.iter().map(|(k, _)| *k).collect();
        listed.sort_unstable();
        assert_eq!(keys, listed, "every setting is in the table");
        let yaml = serde_yaml::to_string(&T::default()).unwrap();
        assert_eq!(serde_yaml::from_str::<T>(&yaml).unwrap(), T::default());
        assert_eq!(serde_yaml::from_str::<T>("{}").unwrap(), T::default());
        for (key, raw) in table {
            let parsed: T = serde_yaml::from_str(&format!("{key}: {raw}")).unwrap();
            assert_ne!(parsed, T::default(), "{key} read from config");
            let back: T = serde_yaml::from_str(&serde_yaml::to_string(&parsed).unwrap()).unwrap();
            assert_eq!(back, parsed, "{key} round-trips");
            let value: serde_json::Value = serde_yaml::from_str(raw).unwrap();
            let (next, loosened) = set(&T::default(), key, value).unwrap();
            assert_eq!(next, parsed, "{key} set");
            let (undo, tightened_loosens) = set(&next, key, defaults[*key].clone()).unwrap();
            assert_eq!(undo, T::default());
            // `allow_reveal` defaults on: turning it off tightens; `timeout_secs` gates nothing.
            match *key {
                "allow_reveal" => assert!(!loosened && tightened_loosens, "{key}"),
                "timeout_secs" => assert!(!loosened && !tightened_loosens, "{key}"),
                _ => assert!(
                    loosened && !tightened_loosens,
                    "{key}: default → {raw} loosens"
                ),
            }
        }
    }

    /// 221 §13 B2 (j), the config half: every D20 key round-trips through YAML at its default and
    /// changed, and each change is classified loosening or tightening.
    #[test]
    fn every_setting_round_trips_and_classifies_loosening() {
        check_keys::<ConnSettings>(CONN);
        check_keys::<ExternalSettings>(HARNESS);
    }

    #[test]
    fn set_refuses_unknown_keys_wrong_types_and_ranges() {
        let s = ConnSettings::default();
        assert!(set(&s, "nope", true.into()).is_err());
        assert!(set(&s, "remember_unlock", "yes".into()).is_err());
        assert!(set(&s, "delete_recovery_days", 3.into()).is_err());
    }

    #[test]
    fn a_connection_keeps_backend_keys_beside_the_settings() {
        let c: ConnectionConfig = serde_yaml::from_str(
            "backend: aws\nprofile: work-admin\nregion: eu-west-1\nallow_reveal: false\n",
        )
        .unwrap();
        assert_eq!(c.backend, "aws");
        assert!(!c.settings.allow_reveal);
        assert_eq!(c.settings.lock_after_idle_secs, 900);
        assert_eq!(
            c.other.keys().map(String::as_str).collect::<Vec<_>>(),
            ["profile", "region"]
        );
        let back: ConnectionConfig =
            serde_yaml::from_str(&serde_yaml::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }
}
