use super::*;

#[rpc(server, client)]
pub trait RoutinesApi {
    /// Persist `schedule.run_timeout_secs` (88); runs fired from then on use it.
    #[method(name = "set_schedule_run_timeout")]
    async fn set_schedule_run_timeout(&self, seconds: u64) -> RpcResult<()>;
    /// Every schedule file with its next/last fire and parse state (34 §8).
    #[method(name = "list_schedules")]
    async fn list_schedules(&self) -> RpcResult<Vec<schedule::ScheduleInfo>>;
    /// Every installed sensor with its `# arg:` header, description and
    /// install state (108 §9.2) for the Routines form and the `routines` skill.
    #[method(name = "list_sensors")]
    async fn list_sensors(&self) -> RpcResult<Vec<schedule::SensorInfo>>;
    /// Fire a schedule now, ignoring `next`; returns its session (34 §8).
    /// Refused while a run is in flight (`busy`) or the file does not parse.
    #[method(name = "fire_schedule")]
    async fn fire_schedule(&self, name: String) -> RpcResult<SessionId>;
    /// Flip a schedule's `enabled:` line (33 §3.5): pause/resume a loop.
    /// `Err` when unknown.
    #[method(name = "enable_schedule")]
    async fn enable_schedule(
        &self,
        name: String,
        enabled: bool,
    ) -> RpcResult<schedule::ScheduleInfo>;
    /// The last `limit` fires and skips of `name`, newest first (34 §8).
    #[method(name = "schedule_history")]
    async fn schedule_history(&self, name: String, limit: u32)
    -> RpcResult<Vec<schedule::RunInfo>>;
    /// A schedule file's front matter and body for the editor. `Err` when unknown.
    #[method(name = "get_schedule")]
    async fn get_schedule(&self, name: String) -> RpcResult<schedule::file::ScheduleFile>;
    /// Validate and write a schedule file (create or replace); returns its live row.
    /// `Err` with the name rule, the parse error, or the io error.
    #[method(name = "put_schedule")]
    async fn put_schedule(
        &self,
        file: schedule::file::ScheduleFile,
    ) -> RpcResult<schedule::ScheduleInfo>;
    /// Remove a schedule file; its history gets a `removed` row. `Err` when unknown.
    #[method(name = "delete_schedule")]
    async fn delete_schedule(&self, name: String) -> RpcResult<()>;
    /// The 40 §1.12 verdict for a `when:` schedule: dry-run its sensor twice,
    /// probe, report `fire?`, hash, flaps, "would fire now". `Err` when unknown
    /// or not a watcher.
    #[method(name = "test_schedule")]
    async fn test_schedule(&self, name: String) -> RpcResult<schedule::Verdict>;
    /// The 40 §1.12 verdict on an unsaved file (108 §9.4): render + parse +
    /// dry-run, nothing written. `Err` with the parse error or `not a watcher`.
    #[method(name = "test_schedule_file")]
    async fn test_schedule_file(
        &self,
        file: schedule::file::ScheduleFile,
    ) -> RpcResult<schedule::Verdict>;
}

#[async_trait]
impl RoutinesApiServer for Server {

    async fn set_schedule_run_timeout(&self, seconds: u64) -> RpcResult<()> {
        send(&schedule()?, schedule::SetRunTimeout { seconds }).await
    }

    async fn list_schedules(&self) -> RpcResult<Vec<schedule::ScheduleInfo>> {
        send(&schedule()?, schedule::ListSchedules).await
    }

    async fn list_sensors(&self) -> RpcResult<Vec<schedule::SensorInfo>> {
        send(&schedule()?, schedule::ListSensors).await
    }

    async fn fire_schedule(&self, name: String) -> RpcResult<SessionId> {
        send(&schedule()?, schedule::Fire { name }).await
    }

    async fn enable_schedule(
        &self,
        name: String,
        enabled: bool,
    ) -> RpcResult<schedule::ScheduleInfo> {
        send(&schedule()?, schedule::SetEnabled { name, enabled }).await
    }

    async fn schedule_history(
        &self,
        name: String,
        limit: u32,
    ) -> RpcResult<Vec<schedule::RunInfo>> {
        send(&schedule()?, schedule::History { name, limit }).await
    }

    async fn get_schedule(&self, name: String) -> RpcResult<schedule::file::ScheduleFile> {
        send(&schedule()?, schedule::Get { name }).await
    }

    async fn put_schedule(
        &self,
        file: schedule::file::ScheduleFile,
    ) -> RpcResult<schedule::ScheduleInfo> {
        send(&schedule()?, schedule::Put(file)).await
    }

    async fn delete_schedule(&self, name: String) -> RpcResult<()> {
        send(&schedule()?, schedule::Delete { name }).await
    }

    async fn test_schedule(&self, name: String) -> RpcResult<schedule::Verdict> {
        send(&schedule()?, schedule::TestSchedule { name }).await
    }

    async fn test_schedule_file(
        &self,
        file: schedule::file::ScheduleFile,
    ) -> RpcResult<schedule::Verdict> {
        send(&schedule()?, schedule::TestFile(file)).await
    }
}

/// Build this domain's RPC module.
pub fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    RoutinesApiServer::into_rpc(server)
}
