use super::*;

#[rpc(server, client)]
pub trait JobsApi {
    /// Background jobs of a session (14 §2): id, state, seconds, log path, tail.
    #[method(name = "list_jobs", with_extensions)]
    async fn list_jobs(&self, session: SessionId) -> RpcResult<Vec<JobView>>;
    /// Every session with a running job, a live PTY or a pending reminder
    /// (ids only): the desktop's sidebar snapshot after connect / resync.
    #[method(name = "list_live_counts")]
    async fn list_live_counts(&self) -> RpcResult<Vec<LiveCountsRow>>;
    /// Kill a running job; `Err` when unknown or already finished.
    #[method(name = "cancel_job", with_extensions)]
    async fn cancel_job(&self, session: SessionId, id: JobId) -> RpcResult<()>;
    /// One page of a PTY job's retained terminal (51 §7): `Unchanged` when
    /// `if_revision` still matches, `Unavailable` when the job's terminal is
    /// gone. An error `terminal busy` means every render slot is taken; retry.
    #[method(name = "pty_screen", with_extensions)]
    async fn pty_screen(
        &self,
        session: SessionId,
        job: JobId,
        viewport: PtyViewport,
        if_revision: Option<u64>,
    ) -> RpcResult<PtyScreenReply>;
    /// A human types into a running PTY job of `session` (55 §2.1): `text`
    /// verbatim, then `keys`, encoded for the program's current modes. Returns
    /// once written; the viewer keeps polling `pty_screen`. Errors when the
    /// job is unknown, finished, or the write fails.
    #[method(name = "pty_input", with_extensions)]
    async fn pty_input(
        &self,
        session: SessionId,
        job: JobId,
        text: Option<String>,
        keys: Option<Vec<Key>>,
    ) -> RpcResult<()>;
    /// Resize a running PTY job's grid (the program gets SIGWINCH). Rows
    /// 2–200, columns 10–400. Errors when the job is unknown or finished.
    #[method(name = "pty_resize", with_extensions)]
    async fn pty_resize(
        &self,
        session: SessionId,
        job: JobId,
        rows: u16,
        cols: u16,
    ) -> RpcResult<()>;
    /// Attach a terminal emulator to a PTY job: first a `reset` item with
    /// the retained buffer as escape sequences (base64), then every byte the
    /// program writes as `data` items, in order, and `exit` once it ends.
    /// `rows`/`cols` resize the pty before the snapshot. A client that falls
    /// behind gets a fresh `reset` instead of a gap.
    #[subscription(name = "subscribe_pty", unsubscribe = "unsubscribe_pty", item = PtyChunk, with_extensions)]
    async fn subscribe_pty(
        &self,
        session: SessionId,
        job: JobId,
        rows: Option<u16>,
        cols: Option<u16>,
    ) -> SubscriptionResult;
}

#[async_trait]
impl JobsApiServer for Server {

    async fn list_jobs(&self, ext: &Extensions, session: SessionId) -> RpcResult<Vec<JobView>> {
        self.authorize(ext, session).await?;
        ask::<Jobs, _>(JOBS_ACTOR, ListJobs(session)).await
    }

    async fn list_live_counts(&self) -> RpcResult<Vec<LiveCountsRow>> {
        ask::<Jobs, _>(JOBS_ACTOR, LiveCounts).await
    }

    /// Every RPC caller is a human surface (TUI `/jobs kill`, the desktop
    /// row): the model learns of the kill by a one-line `<job_result>`.
    async fn cancel_job(&self, ext: &Extensions, session: SessionId, id: JobId) -> RpcResult<()> {
        self.authorize(ext, session).await?;
        ask::<Jobs, _>(
            JOBS_ACTOR,
            CancelJob {
                session,
                id,
                origin: CancelOrigin::Ui,
            },
        )
        .await
    }

    async fn pty_screen(
        &self,
        ext: &Extensions,
        session: SessionId,
        job: JobId,
        viewport: PtyViewport,
        if_revision: Option<u64>,
    ) -> RpcResult<PtyScreenReply> {
        self.authorize(ext, session).await?;
        lookup::<Jobs>(JOBS_ACTOR)?
            .ask(PtyScreen {
                session,
                id: job,
                viewport,
                if_revision,
            })
            .await
            .map_err(|e| match e {
                SendError::HandlerError(busy) => failed(format!("terminal busy: {busy}")),
                other => failed(other.to_string()),
            })
    }

    async fn pty_input(
        &self,
        ext: &Extensions,
        session: SessionId,
        job: JobId,
        text: Option<String>,
        keys: Option<Vec<Key>>,
    ) -> RpcResult<()> {
        self.authorize(ext, session).await?;
        lookup::<Jobs>(JOBS_ACTOR)?
            .ask(PtyInput {
                session,
                id: job,
                text,
                keys: keys.unwrap_or_default(),
            })
            .await
            .map_err(|e| match e {
                SendError::HandlerError(msg) => failed(msg),
                other => failed(other.to_string()),
            })
    }

    async fn pty_resize(
        &self,
        ext: &Extensions,
        session: SessionId,
        job: JobId,
        rows: u16,
        cols: u16,
    ) -> RpcResult<()> {
        self.authorize(ext, session).await?;
        lookup::<Jobs>(JOBS_ACTOR)?
            .ask(PtyResize {
                session,
                id: job,
                rows,
                cols,
            })
            .await
            .map_err(|e| match e {
                SendError::HandlerError(msg) => failed(msg),
                other => failed(other.to_string()),
            })
    }

    async fn subscribe_pty(
        &self,
        pending: PendingSubscriptionSink,
        ext: &Extensions,
        session: SessionId,
        job: JobId,
        rows: Option<u16>,
        cols: Option<u16>,
    ) -> SubscriptionResult {
        let admit = async {
            self.authorize(ext, session).await?;
            attach_pty(session, &job, rows.zip(cols)).await
        };
        let mut at = match admit.await {
            Ok(a) => a,
            Err(e) => {
                pending.reject(e).await;
                return Ok(());
            }
        };
        let sink = pending.accept().await?;
        let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
        let item = |c: &PtyChunk| {
            SubscriptionMessage::from(
                serde_json::value::to_raw_value(c).expect("PtyChunk serializes"),
            )
        };
        tokio::spawn(async move {
            loop {
                let reset = PtyChunk::Reset {
                    data: b64(&at.replay),
                    rows: at.rows,
                    cols: at.cols,
                };
                if sink.send(item(&reset)).await.is_err() {
                    return;
                }
                if let Some(exit) = at.exit {
                    let _ = sink.send(item(&PtyChunk::Exit { exit })).await;
                    return;
                }
                loop {
                    let out = tokio::select! {
                        _ = sink.closed() => return,
                        r = at.rx.recv() => r,
                    };
                    let chunk = match out {
                        Ok(PtyOut::Data(bytes)) => PtyChunk::Data { data: b64(&bytes) },
                        Ok(PtyOut::Exit(exit)) => {
                            let _ = sink.send(item(&PtyChunk::Exit { exit })).await;
                            return;
                        }
                        // Behind by more than the ring: start over from a snapshot.
                        Err(broadcast::error::RecvError::Lagged(_)) => break,
                        Err(broadcast::error::RecvError::Closed) => return,
                    };
                    if sink.send(item(&chunk)).await.is_err() {
                        return;
                    }
                }
                at = match attach_pty(session, &job, None).await {
                    Ok(a) => a,
                    Err(_) => return,
                };
            }
        });
        Ok(())
    }
}
