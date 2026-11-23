//! `share_publish`, `share_list`, `share_stop` (210 §7.1).

use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use share_proto::names;
use tools::{Risk, Tool, ToolCx, ToolOutput};

use crate::config::Expiry;
use crate::engine::{CallOrigin, Consent, Engine};
use crate::model::{Access, Source};

/// The source argument: exactly one of `port` or `dir`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceArg {
    /// A dev server on this machine's loopback.
    pub port: Option<u16>,
    /// An absolute directory to serve as static files.
    pub dir: Option<String>,
}

/// Arguments of `share_publish`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PublishArgs {
    /// `{port: 5173}` or `{dir: "/abs/path"}`.
    pub source: SourceArg,
    /// Sub-label: `<label>.<name>.D`. Default for a port source: 6 random characters.
    pub label: Option<String>,
    /// One of the mesh's held names; default the primary.
    pub name: Option<String>,
    /// Path mount for a `dir` source: `/docs`. Port sources are always a sub-label.
    pub path: Option<String>,
    /// `1h`, `8h` (default), `24h` or `7d`, counted from going live.
    pub expiry: Option<Expiry>,
    /// `public` (default) or `password`: the engine generates the password and shows it to the user.
    pub access: Option<Access>,
}

/// A validated publish: what the approval card shows, before the name and edge are known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// Source.
    pub source: Source,
    /// Sub-label, if any.
    pub label: Option<String>,
    /// Requested name.
    pub name: Option<String>,
    /// `/` or `/docs`.
    pub path: String,
    /// Lifetime.
    pub expiry: Expiry,
    /// Access.
    pub access: Access,
}

/// Validate `share_publish` arguments (§7.1, §5.6): one source; a port source has no path and
/// gets a random label; labels, names and paths obey `share-proto`'s rules.
pub fn plan(
    args: PublishArgs,
    default_expiry: Expiry,
    random: [u8; names::RANDOM_LEN],
) -> Result<Plan, String> {
    let source = match (args.source.port, args.source.dir) {
        (Some(_), Some(_)) => return Err("give either source.port or source.dir, not both".into()),
        (None, None) => return Err("source needs port or dir".into()),
        (Some(0), None) => return Err("port 0 is not a port".into()),
        (Some(p), None) => Source::Port(p),
        (None, Some(d)) => {
            if !d.starts_with('/') {
                return Err("dir must be an absolute path".into());
            }
            Source::Dir(d)
        }
    };
    let path = args.path.unwrap_or_else(|| "/".into());
    names::check_path(&path).map_err(|e| format!("path: {}", e.reason()))?;
    let label = match (&source, args.label) {
        (_, Some(l)) => {
            names::check_label(&l).map_err(|e| format!("label: {}", e.reason()))?;
            Some(l)
        }
        (Source::Port(_), None) => Some(names::random_name(random)),
        (Source::Dir(_), None) => None,
    };
    if matches!(source, Source::Port(_)) && path != "/" {
        return Err("a port source is always a sub-label: use label, not path".into());
    }
    if let Some(n) = &args.name {
        names::check_label(n).map_err(|e| format!("name: {}", e.reason()))?;
    }
    Ok(Plan {
        source,
        label,
        name: args.name,
        path,
        expiry: args.expiry.unwrap_or(default_expiry),
        access: args.access.unwrap_or_default(),
    })
}

/// `share_publish`.
pub struct SharePublishTool {
    /// The engine.
    pub engine: Arc<Engine>,
}

#[async_trait::async_trait]
impl Tool for SharePublishTool {
    const NAME: &'static str = "share_publish";
    const RISK: Risk = Risk::Destructive;
    const RETURNS: &'static str = "dict";
    const DIRECT: bool = true;
    /// §7.2: only the owner, in person, approves a publish.
    const HUMAN_ONLY: bool = true;
    type Args = PublishArgs;

    fn description(&self) -> String {
        "Publish a local dev server (`source: {port}`) or a static directory (`source: {dir}`) at a \
         public https URL on the internet, through the user's sharing edge. Only the owner can \
         approve it, in person; the card shows the exact URL, source, expiry and access. `label` \
         picks the sub-label, `name` one of the mesh's held names, `path` a mount for a dir; \
         `expiry` 1h | 8h | 24h | 7d (default 8h); `access` public | password (the engine makes \
         the password and shows it to the user). Returns {id, url, expires_ms}. Refused while \
         public sharing is off in Settings › Sharing."
            .into()
    }

    fn promotable(&self) -> bool {
        false
    }

    async fn call(&self, args: PublishArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let random = crate::engine::random_bytes();
        let plan = match plan(args, self.engine.cfg.default_expiry, random) {
            Ok(p) => p,
            Err(e) => return Ok(ToolOutput::err(format!("share_publish: {e}"))),
        };
        // The human-only row this call passed (the registry scopes the body with it); none =
        // the gate was bypassed, so refuse (m7).
        let Some(consent) = Consent::of_call() else {
            return Ok(ToolOutput::err(
                "share_publish: refused: this call carries no owner approval",
            ));
        };
        let origin = CallOrigin::Call {
            session: cx.session.to_string(),
            call_id: cx.call_id.clone(),
        };
        match self.engine.publish(plan, origin, consent).await {
            Ok(p) => {
                let mut text = format!("Published {} ({})", p.share.url, p.share.state.as_str());
                if let Some(w) = &p.pending {
                    text.push_str("; it goes live once its certificate is issued");
                    if let Some(q) = w.queue_pos {
                        text.push_str(&format!(", queue position {q}"));
                    }
                    if let Some(eta) = w.eta_s {
                        text.push_str(&format!(", about {eta} s"));
                    }
                }
                if p.password.is_some() {
                    // The password goes to the owner's surfaces (`share_password`), never to
                    // the model.
                    text.push_str(
                        "; password access: the user sees the password in Settings › Sharing, \
                         you do not",
                    );
                }
                Ok(ToolOutput {
                    structured: Some(serde_json::json!({
                        "id": p.share.id, "url": p.share.url, "expires_ms": p.share.expires_ms,
                        "state": p.share.state.as_str(), "pending": p.pending,
                    })),
                    ..ToolOutput::ok(text)
                })
            }
            Err(e) => Ok(ToolOutput::err(format!("share_publish: {e:#}"))),
        }
    }
}

/// Arguments of `share_stop`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StopArgs {
    /// A share id, or `all` to stop every share of the mesh at the edge.
    pub id: String,
}

/// `share_stop`: never gated (§7.1).
pub struct ShareStopTool {
    /// The engine.
    pub engine: Arc<Engine>,
}

#[async_trait::async_trait]
impl Tool for ShareStopTool {
    const NAME: &'static str = "share_stop";
    const RISK: Risk = Risk::ReadOnly;
    type Args = StopArgs;

    fn description(&self) -> String {
        "Stop a public share by id, or `all` to stop every public share of the mesh. Always allowed."
            .into()
    }

    async fn call(&self, args: StopArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let r = if args.id == "all" {
            self.engine.stop_all().await.map(|o| match o.edge_pending {
                None => format!("stopped {} share(s)", o.stopped),
                Some(why) => format!(
                    "stopped {} share(s) on this machine; the mesh-wide stop at the edge is \
                     pending ({why}) and is sent again on reconnect",
                    o.stopped
                ),
            })
        } else {
            self.engine
                .stop(&args.id, crate::model::StoppedBy::User)
                .await
                .map(|_| format!("stopped {}", args.id))
        };
        Ok(match r {
            Ok(t) => ToolOutput::ok(t),
            Err(e) => ToolOutput::err(format!("share_stop: {e:#}")),
        })
    }
}

/// Arguments of `share_list`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListArgs {}

/// `share_list`.
pub struct ShareListTool {
    /// The engine.
    pub engine: Arc<Engine>,
}

#[async_trait::async_trait]
impl Tool for ShareListTool {
    const NAME: &'static str = "share_list";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "list[dict]";
    type Args = ListArgs;

    fn description(&self) -> String {
        "List the mesh's public shares: id, url, state, source, expiry.".into()
    }

    async fn call(&self, _: ListArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let list = self.engine.list().await?;
        let text = if list.is_empty() {
            "no public shares".to_owned()
        } else {
            list.iter()
                .map(|s| format!("{} {} {}", s.id, s.url, s.state.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        };
        Ok(ToolOutput {
            structured: Some(serde_json::to_value(&list)?),
            ..ToolOutput::ok(text)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: serde_json::Value) -> Result<PublishArgs, serde_json::Error> {
        serde_json::from_value(v)
    }

    #[test]
    fn publish_argument_validation() {
        let r = [1u8; 6];
        let p = plan(
            args(serde_json::json!({"source": {"port": 5173}})).unwrap(),
            Expiry::H8,
            r,
        )
        .unwrap();
        assert_eq!(p.source, Source::Port(5173));
        assert_eq!(p.label.as_deref().map(str::len), Some(6));
        assert_eq!(
            (p.path.as_str(), p.expiry, p.access),
            ("/", Expiry::H8, Access::Public)
        );

        let p = plan(
            args(serde_json::json!({"source": {"dir": "/srv/site"}, "path": "/docs", "expiry": "7d", "access": "password"}))
                .unwrap(),
            Expiry::H8,
            r,
        )
        .unwrap();
        assert_eq!(
            (p.label, p.path.as_str(), p.expiry, p.access),
            (None, "/docs", Expiry::D7, Access::Password)
        );

        let bad = |v| plan(args(v).unwrap(), Expiry::H8, r).unwrap_err();
        assert!(bad(serde_json::json!({"source": {}})).contains("port or dir"));
        assert!(bad(serde_json::json!({"source": {"port": 1, "dir": "/x"}})).contains("not both"));
        assert!(bad(serde_json::json!({"source": {"dir": "rel"}})).contains("absolute"));
        assert!(
            bad(serde_json::json!({"source": {"port": 80}, "path": "/x"})).contains("sub-label")
        );
        assert!(
            bad(serde_json::json!({"source": {"dir": "/x"}, "path": "/A"})).starts_with("path")
        );
        assert!(
            bad(serde_json::json!({"source": {"port": 80}, "label": "-x"})).starts_with("label")
        );
        assert!(
            bad(serde_json::json!({"source": {"port": 80}, "name": "xn--a"})).starts_with("name")
        );
        assert!(args(serde_json::json!({"source": {"port": 1}, "expiry": "2h"})).is_err());
        assert!(
            args(serde_json::json!({"source": {"port": 1}, "password": "x"})).is_err(),
            "no password argument"
        );
    }
}
