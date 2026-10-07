//! 125 §6.2 / §6.3: the supervisor side of the advisor — the `ask` targets
//! of `set_advisor`, `advisor_status` and the `set_advisor_*` defaults.
//!
//! Contract wave (C): stubs only, so the RPC layer and every frontend
//! compile against the frozen names. W1 (engine) replaces the bodies with
//! the real handlers (create/dispose/pick through the `Advisors` actor,
//! status from the link, defaults through `write_config_value`).

use kameo::message::{Context, Message};

use super::Agents;
use crate::{AdvisorDefaults, AdvisorStatus, AdvisorStatusOf, SetAdvisor, SetAdvisorDefaults};

/// What every stub answers until W1 lands.
const NOT_BUILT: &str = "advisor: not built";

impl Message<SetAdvisor> for Agents {
    type Reply = Result<AdvisorStatus, String>;

    async fn handle(
        &mut self,
        _: SetAdvisor,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<AdvisorStatus, String> {
        Err(NOT_BUILT.to_owned())
    }
}

impl Message<AdvisorStatusOf> for Agents {
    type Reply = AdvisorStatus;

    async fn handle(
        &mut self,
        m: AdvisorStatusOf,
        _: &mut Context<Self, Self::Reply>,
    ) -> AdvisorStatus {
        AdvisorStatus {
            session: m.session,
            ..AdvisorStatus::default()
        }
    }
}

impl Message<SetAdvisorDefaults> for Agents {
    type Reply = Result<AdvisorDefaults, String>;

    async fn handle(
        &mut self,
        _: SetAdvisorDefaults,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<AdvisorDefaults, String> {
        Err(NOT_BUILT.to_owned())
    }
}
