//! AWS profile names, never credentials. The SDK owns file locations, parsing,
//! merging config/credentials, and the ambient profile selection (02 §2.2).

use aws_config::profile::{ProfileSet, load};
use serde::{Deserialize, Serialize};

/// Profiles available on the harness that performs inference.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AwsProfiles {
    /// Sorted, deduplicated SDK profile names (not SSO-session or service sections).
    pub profiles: Vec<String>,
    /// Explicit harness-wide inference selection; `None` uses the default chain.
    pub selected: Option<String>,
    /// SDK-selected ambient profile (`AWS_PROFILE`, otherwise `default`).
    pub default_profile: String,
}

/// `ask` → `Result<AwsProfiles, String>`; rereads AWS files without resolving credentials.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ListAwsProfiles;

/// `ask` → `Result<AwsProfiles, String>`; persisted, effective on the next Bedrock call.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetAwsProfile {
    /// A discovered profile, or `None` to restore the AWS default credential chain.
    pub profile: Option<String>,
}

impl AwsProfiles {
    pub(crate) async fn load(selected: Option<String>) -> Result<Self, String> {
        let profiles = load(
            &Default::default(),
            &Default::default(),
            &Default::default(),
            None,
        )
        .await
        .map_err(|e| format!("AWS profiles: {e}"))?;
        Ok(Self::from_sdk(&profiles, selected))
    }

    fn from_sdk(set: &ProfileSet, selected: Option<String>) -> Self {
        let mut profiles: Vec<String> = set.profiles().map(str::to_owned).collect();
        profiles.sort_unstable();
        Self {
            profiles,
            selected,
            default_profile: set.selected_profile().to_owned(),
        }
    }

    pub(crate) fn select(&mut self, profile: Option<String>) -> Result<(), String> {
        if let Some(name) = &profile
            && !self.profiles.contains(name)
        {
            return Err(format!(
                "AWS profile {name:?} was not found on this harness"
            ));
        }
        self.selected = profile;
        Ok(())
    }
}

/// Whether the SDK's ambient profile (`AWS_PROFILE`, else `default`) has a section in
/// `AWS_SHARED_CREDENTIALS_FILE` (`~/.aws/credentials`) or `AWS_CONFIG_FILE` (`~/.aws/config`).
/// Sync and SDK-free so the model catalog's `configured` predicate can call it; a plain
/// header scan, since the SDK will do the real parse when a call is made.
pub(crate) fn ambient_profile_exists() -> bool {
    let name = std::env::var("AWS_PROFILE")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "default".into());
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let file = |env: &str, default: &str| {
        std::env::var_os(env)
            .filter(|v| !v.is_empty())
            .map(std::path::PathBuf::from)
            .or_else(|| home.as_ref().map(|h| h.join(".aws").join(default)))
    };
    has_profile(
        file("AWS_SHARED_CREDENTIALS_FILE", "credentials"),
        file("AWS_CONFIG_FILE", "config"),
        &name,
    )
}

/// `[name]` in the credentials file, or `[name]` / `[profile name]` in the config file.
fn has_profile(
    credentials: Option<std::path::PathBuf>,
    config: Option<std::path::PathBuf>,
    name: &str,
) -> bool {
    let section = |path: Option<std::path::PathBuf>, headers: &[String]| {
        path.and_then(|p| std::fs::read_to_string(p).ok())
            .is_some_and(|text| {
                text.lines()
                    .map(str::trim)
                    .any(|line| headers.iter().any(|h| h == line))
            })
    };
    section(credentials, &[format!("[{name}]")])
        || section(config, &[format!("[{name}]"), format!("[profile {name}]")])
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_types::os_shim_internal::{Env, Fs};

    #[tokio::test]
    async fn sdk_profiles_merge_and_selection_rejects_unknown_names() {
        let fs = Fs::from_slice(&[
            (
                "/config",
                "[profile work]\nregion = eu-west-1\n[profile personal]\nregion = us-west-2\n[sso-session login]\nsso_region = eu-west-1\n[services local]\n",
            ),
            (
                "/credentials",
                "[work]\naws_access_key_id = test\n[credentials-only]\naws_access_key_id = test\n",
            ),
        ]);
        let env = Env::from_slice(&[
            ("AWS_CONFIG_FILE", "/config"),
            ("AWS_SHARED_CREDENTIALS_FILE", "/credentials"),
            ("AWS_PROFILE", "work"),
        ]);
        let set = load(&fs, &env, &Default::default(), None).await.unwrap();
        let mut profiles = AwsProfiles::from_sdk(&set, None);
        assert_eq!(profiles.profiles, ["credentials-only", "personal", "work"]);
        assert_eq!(profiles.default_profile, "work");
        profiles.select(Some("personal".into())).unwrap();
        assert!(profiles.select(Some("missing".into())).is_err());
        assert_eq!(profiles.selected.as_deref(), Some("personal"));
        profiles.select(None).unwrap();
        assert!(profiles.selected.is_none());
    }

    #[test]
    fn ambient_profile_is_a_section_header_in_either_file() {
        let scratch = crate::TestDir::new("aws");
        let dir = scratch.0.clone();
        let creds = dir.join("credentials");
        let config = dir.join("config");
        std::fs::write(&creds, "[work]\naws_access_key_id = x\n").unwrap();
        std::fs::write(
            &config,
            "[profile personal]\nregion = us-west-2\n[default]\nregion = eu-west-1\n",
        )
        .unwrap();
        let both = || (Some(creds.clone()), Some(config.clone()));
        assert!(has_profile(both().0, both().1, "work"));
        assert!(has_profile(both().0, both().1, "personal"));
        assert!(has_profile(both().0, both().1, "default"));
        assert!(!has_profile(both().0, both().1, "missing"));
        // No files at all (a machine without ~/.aws): nothing is configured.
        assert!(!has_profile(Some(dir.join("nope")), None, "default"));
    }
}
