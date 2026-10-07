//! Launch at login for the nested companion (130 R1-2, D12, §5.7). `SMAppService.loginItem(identifier:)`
//! must be called from the MAIN app's process, so the companion only writes its wish to
//! `<data_dir>/companion.json` (`{ "launch_at_login": true|false }`, the Settings toggle) and the shell
//! registers or unregisters the helper at start. `kSMErrorAlreadyRegistered` is success. While the item is
//! `enabled`, launchd starts the helper and [`super::init`] must not also open it.
//!
//! Raw `msg_send!`: `objc2-service-management` is not in the vendored registry.

use std::path::PathBuf;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use objc2_foundation::{NSError, NSString};

use super::BUNDLE_ID;

#[link(name = "ServiceManagement", kind = "framework")]
unsafe extern "C" {}

/// `SMErrors.h`: `kSMErrorInternalFailure = 2`, then nine more in order; `kSMErrorAlreadyRegistered` is the last.
const K_SM_ERROR_ALREADY_REGISTERED: isize = 11;
/// Written by the companion's Settings window; absent = off.
pub const FILE: &str = "companion.json";

/// `SMAppServiceStatus` plus `unavailable` (pre-13 / no bundle context).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginItemStatus {
    NotRegistered,
    Enabled,
    /// The user must flip the switch in System Settings › Login Items.
    RequiresApproval,
    NotFound,
    Unavailable,
}

impl LoginItemStatus {
    fn from_raw(raw: isize) -> Self {
        match raw {
            0 => Self::NotRegistered,
            1 => Self::Enabled,
            2 => Self::RequiresApproval,
            3 => Self::NotFound,
            _ => Self::Unavailable,
        }
    }

    /// snake_case for `companion_login_item_status`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotRegistered => "not_registered",
            Self::Enabled => "enabled",
            Self::RequiresApproval => "requires_approval",
            Self::NotFound => "not_found",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(serde::Deserialize, Default)]
struct Wish {
    #[serde(default)]
    launch_at_login: bool,
}

fn file() -> PathBuf {
    crate::launch::data_dir().join(FILE)
}

/// The toggle's last value; a missing or unreadable file is `false`.
pub fn wanted() -> bool {
    std::fs::read(file())
        .ok()
        .and_then(|b| serde_json::from_slice::<Wish>(&b).ok())
        .unwrap_or_default()
        .launch_at_login
}

fn service() -> Retained<AnyObject> {
    let id = NSString::from_str(BUNDLE_ID);
    // SAFETY: `+[SMAppService loginItemServiceWithIdentifier:]` takes an NSString and returns an instance.
    unsafe { msg_send![class!(SMAppService), loginItemServiceWithIdentifier: &*id] }
}

/// Live `SMAppService.status` for the companion.
pub fn status() -> LoginItemStatus {
    // SAFETY: `-[SMAppService status]` returns an NSInteger.
    let raw: isize = unsafe { msg_send![&*service(), status] };
    LoginItemStatus::from_raw(raw)
}

/// Reconcile launchd with the toggle: register when wanted (already registered is fine), unregister
/// otherwise when something is registered. Returns the status afterwards. Logs `requires_approval`.
pub fn sync() -> LoginItemStatus {
    let svc = service();
    let before = status();
    let wanted = wanted();
    if wanted {
        // SAFETY: `-registerAndReturnError:` returns BOOL with an NSError out-param.
        let res: Result<(), Retained<NSError>> =
            unsafe { msg_send![&*svc, registerAndReturnError: _] };
        match res {
            Ok(()) => tracing::info!("companion: login item registered"),
            Err(e) if e.code() == K_SM_ERROR_ALREADY_REGISTERED => {
                tracing::debug!("companion: login item already registered")
            }
            Err(e) => tracing::warn!(
                "companion: login item register: {}",
                e.localizedDescription()
            ),
        }
    } else if before != LoginItemStatus::NotRegistered && before != LoginItemStatus::Unavailable {
        // SAFETY: as above.
        let res: Result<(), Retained<NSError>> =
            unsafe { msg_send![&*svc, unregisterAndReturnError: _] };
        match res {
            Ok(()) => tracing::info!("companion: login item unregistered"),
            Err(e) => tracing::warn!(
                "companion: login item unregister: {}",
                e.localizedDescription()
            ),
        }
    }
    let after = status();
    if after == LoginItemStatus::RequiresApproval {
        tracing::warn!("companion: login item requires approval in System Settings › Login Items");
    }
    tracing::info!("companion: login item wanted={wanted} status={after:?}");
    after
}
