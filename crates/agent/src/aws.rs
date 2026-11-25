//! AWS profile messages for the Bedrock picker. The profile list itself lives in
//! `aws-profiles` (221 B1), shared with `secrets-aws`.

use serde::{Deserialize, Serialize};

pub use aws_profiles::AwsProfiles;
pub(crate) use aws_profiles::ambient_profile_exists;

/// `ask` → `Result<AwsProfiles, String>`; rereads AWS files without resolving credentials.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ListAwsProfiles;

/// `ask` → `Result<AwsProfiles, String>`; persisted, effective on the next Bedrock call.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetAwsProfile {
    /// A discovered profile, or `None` to restore the AWS default credential chain.
    pub profile: Option<String>,
}
