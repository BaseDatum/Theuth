use super::*;

#[rpc(server, client)]
pub trait KernelsApi {
    // 171:C cells (design 171 §15.2). Bodies: `crate::cells` (E1), `crate::cells_latest` (E2).
    /// 171: run one cell. Press guard (§12.1 inv. 1) + `authorize`; idempotent per `run`.
    /// `confirmed: true` is the user's second press past a warning (§7.1, §7.6): Run anyway
    /// past an unarmed `after` gate, or a danger cell's line/selection ►; omitted or `false`,
    /// both are refused as before.
    #[method(name = "cell_run", with_extensions)]
    async fn cell_run(
        &self,
        cell: cells::CellRef,
        seen: String,
        mode: cells::RunMode,
        run: cells::RunId,
        edited: Option<String>,
        member_ack: Option<String>,
        confirmed: Option<bool>,
    ) -> RpcResult<cells::CellRun>;
    /// 171: Run all as an engine pass (§7.2); returns the pass id.
    #[method(name = "cell_run_all", with_extensions)]
    async fn cell_run_all(
        &self,
        scope: cells::Scope,
        cells: Vec<(cells::CellRef, String)>,
        pass: String,
    ) -> RpcResult<String>;
    /// 171: Stop (`kill: false`) or Kill a run; a queued run is cancelled.
    #[method(name = "cell_stop", with_extensions)]
    async fn cell_stop(&self, run: cells::RunId, kill: bool) -> RpcResult<()>;
    /// 171: bytes (base64) typed into a run's live terminal / its `input()` answer.
    #[method(name = "cell_input", with_extensions)]
    async fn cell_input(&self, run: cells::RunId, data: String) -> RpcResult<()>;
    /// 171: Ask theuth — wake the session with that run's result.
    #[method(name = "cell_ask", with_extensions)]
    async fn cell_ask(&self, run: cells::RunId) -> RpcResult<()>;
    /// 171: a run's live terminal from mark C (`reset`, `data`…, `exit`).
    #[subscription(name = "cell_attach", unsubscribe = "cell_detach", item = PtyChunk, with_extensions)]
    async fn cell_attach(&self, run: cells::RunId) -> SubscriptionResult;
    /// 171: problems, chips, armed/superseded/stale per cell and the scope's kernels.
    #[method(name = "cell_state", with_extensions)]
    async fn cell_state(
        &self,
        scope: cells::Scope,
        cells: Vec<cells::CellRef>,
    ) -> RpcResult<cells::StateReply>;
    /// 171: one run's record and outputs (text capped).
    #[method(name = "cell_run_get", with_extensions)]
    async fn cell_run_get(&self, run: cells::RunId) -> RpcResult<cells::RunGet>;
    /// 171: a page of a run's cleaned (or `ansi`) text.
    #[method(name = "cell_output", with_extensions)]
    async fn cell_output(
        &self,
        run: cells::RunId,
        offset: u64,
        limit: u32,
        ansi: bool,
    ) -> RpcResult<cells::OutputPage>;
    /// 171: one file of a run (images), base64.
    #[method(name = "cell_output_file", with_extensions)]
    async fn cell_output_file(&self, run: cells::RunId, name: String) -> RpcResult<String>;
    /// 171: a scope's runs, newest first.
    #[method(name = "cell_runs", with_extensions)]
    async fn cell_runs(&self, scope: cells::Scope, limit: u32) -> RpcResult<Vec<cells::CellRun>>;
    /// 171: interpreter candidates for `cwd` on this engine or the named one (§6.4).
    #[method(name = "cell_interpreters", with_extensions)]
    async fn cell_interpreters(
        &self,
        cwd: String,
        engine: Option<String>,
    ) -> RpcResult<Vec<cells::Interpreter>>;
    /// 171: Restart a kernel (Close; the next run starts a fresh one).
    #[method(name = "cell_kernel_restart", with_extensions)]
    async fn cell_kernel_restart(&self, kernel: cells::KernelId) -> RpcResult<()>;
    /// 171: Close a kernel.
    #[method(name = "cell_kernel_close", with_extensions)]
    async fn cell_kernel_close(&self, kernel: cells::KernelId) -> RpcResult<()>;
    /// 171: re-capture the login environment.
    #[method(name = "cell_env_refresh", with_extensions)]
    async fn cell_env_refresh(&self) -> RpcResult<()>;
    /// 171: runs in flight on this engine (quit / `t update` sheet).
    #[method(name = "cell_live", with_extensions)]
    async fn cell_live(&self) -> RpcResult<Vec<cells::CellRun>>;
    /// 171: a note's replicated latest runs (§13.2); owner, member or guest.
    #[method(name = "cell_latest", with_extensions)]
    async fn cell_latest(&self, doc: document::DocId) -> RpcResult<Vec<cells::CellLatest>>;
    /// 171F B9: the note a chat reply was opened as, if any; owner, not guest.
    #[method(name = "cell_reply_note_get", with_extensions)]
    async fn cell_reply_note_get(
        &self,
        session: cells::SessionId,
        entry: String,
    ) -> RpcResult<Option<cells::DocId>>;
    /// 171F B9: remember that reply `entry` of `session` was opened as note `doc`.
    #[method(name = "cell_reply_note_set", with_extensions)]
    async fn cell_reply_note_set(
        &self,
        session: cells::SessionId,
        entry: String,
        doc: cells::DocId,
    ) -> RpcResult<()>;
}

#[async_trait]
impl KernelsApiServer for Server {

    // 171:C cells (design 171 §15.2): one delegating line per method.
    async fn cell_run(
        &self,
        ext: &Extensions,
        cell: cells::CellRef,
        seen: String,
        mode: cells::RunMode,
        run: cells::RunId,
        edited: Option<String>,
        member_ack: Option<String>,
        confirmed: Option<bool>,
    ) -> RpcResult<cells::CellRun> {
        crate::cells::run(
            self, ext, cell, seen, mode, run, edited, member_ack, confirmed,
        )
        .await
    }

    async fn cell_run_all(
        &self,
        ext: &Extensions,
        scope: cells::Scope,
        cells: Vec<(cells::CellRef, String)>,
        pass: String,
    ) -> RpcResult<String> {
        crate::cells::run_all(self, ext, scope, cells, pass).await
    }

    async fn cell_stop(&self, ext: &Extensions, run: cells::RunId, kill: bool) -> RpcResult<()> {
        crate::cells::stop(self, ext, run, kill).await
    }

    async fn cell_input(&self, ext: &Extensions, run: cells::RunId, data: String) -> RpcResult<()> {
        crate::cells::input(self, ext, run, data).await
    }

    async fn cell_ask(&self, ext: &Extensions, run: cells::RunId) -> RpcResult<()> {
        crate::cells::ask(self, ext, run).await
    }

    async fn cell_attach(
        &self,
        pending: PendingSubscriptionSink,
        ext: &Extensions,
        run: cells::RunId,
    ) -> SubscriptionResult {
        crate::cells::attach(self, pending, ext, run).await
    }

    async fn cell_state(
        &self,
        ext: &Extensions,
        scope: cells::Scope,
        cells: Vec<cells::CellRef>,
    ) -> RpcResult<cells::StateReply> {
        crate::cells::state(self, ext, scope, cells).await
    }

    async fn cell_run_get(&self, ext: &Extensions, run: cells::RunId) -> RpcResult<cells::RunGet> {
        crate::cells::run_get(self, ext, run).await
    }

    async fn cell_output(
        &self,
        ext: &Extensions,
        run: cells::RunId,
        offset: u64,
        limit: u32,
        ansi: bool,
    ) -> RpcResult<cells::OutputPage> {
        crate::cells::output(self, ext, run, offset, limit, ansi).await
    }

    async fn cell_output_file(
        &self,
        ext: &Extensions,
        run: cells::RunId,
        name: String,
    ) -> RpcResult<String> {
        crate::cells::output_file(self, ext, run, name).await
    }

    async fn cell_runs(
        &self,
        ext: &Extensions,
        scope: cells::Scope,
        limit: u32,
    ) -> RpcResult<Vec<cells::CellRun>> {
        crate::cells::runs(self, ext, scope, limit).await
    }

    async fn cell_interpreters(
        &self,
        ext: &Extensions,
        cwd: String,
        engine: Option<String>,
    ) -> RpcResult<Vec<cells::Interpreter>> {
        crate::cells::interpreters(self, ext, cwd, engine).await
    }

    async fn cell_kernel_restart(
        &self,
        ext: &Extensions,
        kernel: cells::KernelId,
    ) -> RpcResult<()> {
        crate::cells::kernel_restart(self, ext, kernel).await
    }

    async fn cell_kernel_close(&self, ext: &Extensions, kernel: cells::KernelId) -> RpcResult<()> {
        crate::cells::kernel_close(self, ext, kernel).await
    }

    async fn cell_env_refresh(&self, ext: &Extensions) -> RpcResult<()> {
        crate::cells::env_refresh(self, ext).await
    }

    async fn cell_live(&self, ext: &Extensions) -> RpcResult<Vec<cells::CellRun>> {
        crate::cells::live(self, ext).await
    }

    async fn cell_latest(
        &self,
        ext: &Extensions,
        doc: document::DocId,
    ) -> RpcResult<Vec<cells::CellLatest>> {
        crate::cells_latest::latest(self, ext, doc).await
    }

    async fn cell_reply_note_get(
        &self,
        ext: &Extensions,
        session: cells::SessionId,
        entry: String,
    ) -> RpcResult<Option<cells::DocId>> {
        crate::cells::reply_note_get(self, ext, session, entry).await
    }

    async fn cell_reply_note_set(
        &self,
        ext: &Extensions,
        session: cells::SessionId,
        entry: String,
        doc: cells::DocId,
    ) -> RpcResult<()> {
        crate::cells::reply_note_set(self, ext, session, entry, doc).await
    }
}
