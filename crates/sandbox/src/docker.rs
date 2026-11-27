//! The `docker` backend (64 §4.2): a container on this machine running
//! `t --headless` from `deploy/computer/Dockerfile` (release asset) or
//! `Dockerfile.dev` (a locally built Linux `t`). Labels mark the container as
//! ours and pin the image it was created from; a container with our labels
//! and a different image or schema is removed (volumes kept) and recreated.
//! Named volumes for `/data` and `/workspace`; no published ports; restart
//! `unless-stopped`.

use std::collections::HashMap;
use std::path::PathBuf;

use bollard::Docker;
use bollard::errors::Error as DockerError;
use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use bollard::models::{
    ContainerCreateBody, ContainerSummaryStateEnum, HostConfig, RestartPolicy,
    RestartPolicyNameEnum,
};
use bollard::query_parameters::{
    BuildImageOptionsBuilder, CreateContainerOptionsBuilder, CreateImageOptionsBuilder,
    ListContainersOptionsBuilder, RemoveContainerOptionsBuilder, RemoveVolumeOptions,
    StartContainerOptions, UploadToContainerOptionsBuilder,
};
use bytes::Bytes;
use futures::StreamExt;
use tokio::io::AsyncWriteExt;

use crate::{Cmd, DaemonKind, ExecOutput, Handle, Progress, Sandbox};

/// Label naming the engine this container is.
pub const LABEL_ENGINE: &str = "io.theuth.engine";
/// Label pinning the image id the container was created from.
pub const LABEL_IMAGE: &str = "io.theuth.image";
/// Label carrying the container layout version.
pub const LABEL_SCHEMA: &str = "io.theuth.schema";
/// Current layout version.
pub const SCHEMA: &str = "1";

const DOCKERFILE: &str = include_str!("../../../deploy/computer/Dockerfile");
const DOCKERFILE_DEV: &str = include_str!("../../../deploy/computer/Dockerfile.dev");
/// The release image's entrypoint and the install/pair script it shares with native VM nodes
/// (224 §5, §9): the newest signed `t` into /data/bin, then the mesh join.
const RELEASE_FILES: [(&str, &[u8]); 2] = [
    (
        "entrypoint.sh",
        include_bytes!("../../../deploy/computer/entrypoint.sh"),
    ),
    (
        "theuth-install.sh",
        include_bytes!("../../../deploy/computer/theuth-install.sh"),
    ),
];

enum Image {
    /// A pre-built ref, pulled when absent.
    Ref(String),
    /// `deploy/computer/Dockerfile` for this binary's version.
    Release,
    /// `deploy/computer/Dockerfile.dev` with this file as `t`.
    Dev(PathBuf),
}

/// One container.
pub struct DockerSandbox {
    docker: Docker,
    name: String,
    image: Image,
}

impl DockerSandbox {
    /// Connect to the daemon: `DOCKER_HOST`, else Docker Desktop's
    /// `~/.docker/run/docker.sock`, else `/var/run/docker.sock`. `image` is a
    /// pre-built ref; `local_t` selects the dev Dockerfile; neither → the
    /// release Dockerfile.
    pub async fn connect(
        name: &str,
        image: Option<String>,
        local_t: Option<PathBuf>,
    ) -> anyhow::Result<Self> {
        let docker = if std::env::var_os("DOCKER_HOST").is_some() {
            Docker::connect_with_defaults()?
        } else {
            Self::connect_local()?
        };
        docker
            .ping()
            .await
            .map_err(|e| anyhow::anyhow!("docker daemon: {e}"))?;
        let image = match (image, local_t) {
            (Some(r), _) => Image::Ref(r),
            (None, Some(p)) => Image::Dev(p),
            (None, None) => Image::Release,
        };
        Ok(Self {
            docker,
            name: name.to_owned(),
            image,
        })
    }

    /// No `DOCKER_HOST`: Docker Desktop's per-user socket when it exists, else the platform default
    /// (`/var/run/docker.sock`; the `//./pipe/docker_engine` named pipe on Windows, where bollard has
    /// no `connect_with_unix`).
    #[cfg(unix)]
    fn connect_local() -> Result<Docker, bollard::errors::Error> {
        let desktop = dirs::home_dir().map(|h| h.join(".docker/run/docker.sock"));
        match desktop.filter(|p| p.exists()) {
            Some(p) => {
                Docker::connect_with_unix(&p.to_string_lossy(), 120, bollard::API_DEFAULT_VERSION)
            }
            None => Docker::connect_with_local_defaults(),
        }
    }

    #[cfg(not(unix))]
    fn connect_local() -> Result<Docker, bollard::errors::Error> {
        Docker::connect_with_local_defaults()
    }

    /// `theuth-engine-<name>`.
    pub fn container(&self) -> String {
        format!("theuth-engine-{}", self.name)
    }

    fn volumes(&self) -> [String; 2] {
        [
            format!("theuth-{}-data", self.name),
            format!("theuth-{}-workspace", self.name),
        ]
    }

    async fn image_id(&self, r: &str) -> anyhow::Result<Option<String>> {
        match self.docker.inspect_image(r).await {
            Ok(i) => Ok(i.id),
            Err(DockerError::DockerResponseServerError {
                status_code: 404, ..
            }) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Pull or build as needed; the image id.
    async fn ensure_image(&self, out: &Progress) -> anyhow::Result<String> {
        let (tag, context) = match &self.image {
            Image::Ref(r) => {
                if let Some(id) = self.image_id(r).await? {
                    out.say("image", format!("{r} present"));
                    return Ok(id);
                }
                out.say("image", format!("pulling {r}"));
                let opts = CreateImageOptionsBuilder::new().from_image(r).build();
                let mut s = self.docker.create_image(Some(opts), None, None);
                while let Some(item) = s.next().await {
                    item.map_err(|e| anyhow::anyhow!("pull {r}: {e}"))?;
                }
                return self
                    .image_id(r)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("pull {r}: image missing after pull"));
            }
            Image::Release => {
                let tag = format!("theuth-engine:{}", platform::version::get());
                (tag, tar_context(DOCKERFILE, &RELEASE_FILES))
            }
            Image::Dev(p) => {
                let t = std::fs::read(p).map_err(|e| anyhow::anyhow!("{}: {e}", p.display()))?;
                let short = platform::sha256::hex(&t)[..12].to_owned();
                (
                    format!("theuth-engine:dev-{short}"),
                    tar_context(DOCKERFILE_DEV, &[("t", &t)]),
                )
            }
        };
        if let Some(id) = self.image_id(&tag).await? {
            out.say("image", format!("{tag} present"));
            return Ok(id);
        }
        out.say("image", format!("building {tag}"));
        let args = HashMap::from([
            (
                "THEUTH_VERSION".to_owned(),
                platform::version::get().to_owned(),
            ),
            ("THEUTH_PUBKEY".to_owned(), update::PUBKEY.trim().to_owned()),
        ]);
        let opts = BuildImageOptionsBuilder::new()
            .dockerfile("Dockerfile")
            .t(&tag)
            .buildargs(&args)
            .rm(true)
            .build();
        let mut s = self
            .docker
            .build_image(opts, None, Some(bollard::body_full(context.into())));
        while let Some(item) = s.next().await {
            let info = item.map_err(|e| anyhow::anyhow!("build {tag}: {e}"))?;
            if let Some(line) = info.stream {
                let line = line.trim_end();
                if !line.is_empty() {
                    out.say("image", line);
                }
            }
            if let Some(d) = info.error_detail {
                anyhow::bail!("build {tag}: {}", d.message.unwrap_or_default());
            }
        }
        self.image_id(&tag)
            .await?
            .ok_or_else(|| anyhow::anyhow!("build {tag}: no image after build"))
    }

    /// Our container for this name, if any: `(id, running, matches image+schema)`.
    async fn existing(&self, image_id: &str) -> anyhow::Result<Option<(String, bool, bool)>> {
        let filters = HashMap::from([(
            "label".to_owned(),
            vec![format!("{LABEL_ENGINE}={}", self.name)],
        )]);
        let opts = ListContainersOptionsBuilder::new()
            .all(true)
            .filters(&filters)
            .build();
        let list = self.docker.list_containers(Some(opts)).await?;
        Ok(list.into_iter().next().and_then(|c| {
            let id = c.id?;
            let running = matches!(c.state, Some(ContainerSummaryStateEnum::RUNNING));
            let labels = c.labels.unwrap_or_default();
            let matches = labels.get(LABEL_SCHEMA).is_some_and(|s| s == SCHEMA)
                && labels.get(LABEL_IMAGE).is_some_and(|i| i == image_id);
            Some((id, running, matches))
        }))
    }
}

/// A build context: `Dockerfile` and `files` (each executable).
fn tar_context(dockerfile: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut ar = tar::Builder::new(Vec::new());
    let mut add = |name: &str, bytes: &[u8], mode: u32| {
        let mut h = tar::Header::new_gnu();
        h.set_size(bytes.len() as u64);
        h.set_mode(mode);
        h.set_cksum();
        ar.append_data(&mut h, name, bytes).expect("in-memory tar");
    };
    add("Dockerfile", dockerfile.as_bytes(), 0o644);
    for (name, bytes) in files {
        add(name, bytes, 0o755);
    }
    ar.into_inner().expect("in-memory tar")
}

/// A one-file tar for `upload_to_container`, owned by uid 1000 (the image's user).
fn tar_file(name: &str, bytes: &[u8], mode: u32) -> Vec<u8> {
    let mut ar = tar::Builder::new(Vec::new());
    let mut h = tar::Header::new_gnu();
    h.set_size(bytes.len() as u64);
    h.set_mode(mode);
    h.set_uid(1000);
    h.set_gid(1000);
    h.set_cksum();
    ar.append_data(&mut h, name, bytes).expect("in-memory tar");
    ar.into_inner().expect("in-memory tar")
}

#[async_trait::async_trait]
impl Sandbox for DockerSandbox {
    fn kind(&self) -> &'static str {
        "docker"
    }

    async fn create(&self, out: &Progress) -> anyhow::Result<Handle> {
        let image_id = self.ensure_image(out).await?;
        let cname = self.container();
        if let Some((id, running, matches)) = self.existing(&image_id).await? {
            if matches {
                if !running {
                    self.docker
                        .start_container(&id, None::<StartContainerOptions>)
                        .await?;
                }
                out.say("create", format!("{cname} adopted"));
                return Ok(Handle { id });
            }
            out.say(
                "create",
                format!("{cname} is from another image; recreating (volumes kept)"),
            );
            self.docker
                .remove_container(
                    &id,
                    Some(RemoveContainerOptionsBuilder::new().force(true).build()),
                )
                .await?;
        }
        let [data, workspace] = self.volumes();
        let body = ContainerCreateBody {
            image: Some(image_id.clone()),
            env: Some(vec!["THEUTH_DATA_DIR=/data".into()]),
            labels: Some(HashMap::from([
                (LABEL_ENGINE.to_owned(), self.name.clone()),
                (LABEL_SCHEMA.to_owned(), SCHEMA.to_owned()),
                (LABEL_IMAGE.to_owned(), image_id),
            ])),
            host_config: Some(HostConfig {
                binds: Some(vec![
                    format!("{data}:/data"),
                    format!("{workspace}:/workspace"),
                ]),
                restart_policy: Some(RestartPolicy {
                    name: Some(RestartPolicyNameEnum::UNLESS_STOPPED),
                    maximum_retry_count: None,
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        let opts = CreateContainerOptionsBuilder::new().name(&cname).build();
        let created = self.docker.create_container(Some(opts), body).await?;
        self.docker
            .start_container(&created.id, None::<StartContainerOptions>)
            .await?;
        out.say("create", format!("{cname} started"));
        Ok(Handle { id: created.id })
    }

    async fn exec(&self, h: &Handle, cmd: Cmd) -> anyhow::Result<ExecOutput> {
        let timeout = cmd.timeout;
        let line = cmd.line.clone();
        let fut = async {
            let exec = self
                .docker
                .create_exec(
                    &h.id,
                    CreateExecOptions {
                        cmd: Some(vec!["/bin/sh".to_owned(), "-lc".to_owned(), cmd.line]),
                        attach_stdout: Some(true),
                        attach_stderr: Some(true),
                        attach_stdin: Some(cmd.stdin.is_some()),
                        ..Default::default()
                    },
                )
                .await?;
            let mut out = ExecOutput {
                status: -1,
                ..Default::default()
            };
            if let StartExecResults::Attached {
                mut output,
                mut input,
            } = self
                .docker
                .start_exec(&exec.id, None::<StartExecOptions>)
                .await?
            {
                let feed = tokio::spawn(async move {
                    if let Some(bytes) = cmd.stdin {
                        let _ = input.write_all(&bytes).await;
                    }
                    let _ = input.shutdown().await;
                });
                while let Some(item) = output.next().await {
                    match item? {
                        bollard::container::LogOutput::StdOut { message } => {
                            out.stdout.extend_from_slice(&message)
                        }
                        bollard::container::LogOutput::StdErr { message } => {
                            out.stderr.extend_from_slice(&message)
                        }
                        _ => {}
                    }
                }
                feed.await?;
            }
            let ins = self.docker.inspect_exec(&exec.id).await?;
            out.status = ins.exit_code.map_or(-1, |c| c as i32);
            anyhow::Ok(out)
        };
        tokio::time::timeout(timeout, fut)
            .await
            .map_err(|_| anyhow::anyhow!("timed out after {}s: {line}", timeout.as_secs()))?
    }

    async fn put(&self, h: &Handle, path: &str, bytes: Bytes, mode: u32) -> anyhow::Result<()> {
        let (dir, name) = path.rsplit_once('/').unwrap_or((".", path));
        let mk = self
            .exec(
                h,
                Cmd::new(format!("mkdir -p {}", platform::shell::single_quote(dir))),
            )
            .await?;
        if !mk.ok() {
            anyhow::bail!("put {path}: {}", mk.reason());
        }
        let opts = UploadToContainerOptionsBuilder::new().path(dir).build();
        self.docker
            .upload_to_container(
                &h.id,
                Some(opts),
                bollard::body_full(tar_file(name, &bytes, mode).into()),
            )
            .await?;
        Ok(())
    }

    fn durable_roots(&self) -> Vec<String> {
        vec!["/data".into(), "/workspace".into()]
    }

    fn daemon(&self) -> DaemonKind {
        DaemonKind::Entrypoint
    }

    async fn destroy(&self, h: &Handle, preserve_data: bool) -> anyhow::Result<()> {
        match self
            .docker
            .remove_container(
                &h.id,
                Some(RemoveContainerOptionsBuilder::new().force(true).build()),
            )
            .await
        {
            Ok(())
            | Err(DockerError::DockerResponseServerError {
                status_code: 404, ..
            }) => {}
            Err(e) => return Err(e.into()),
        }
        if !preserve_data {
            for v in self.volumes() {
                match self
                    .docker
                    .remove_volume(&v, None::<RemoveVolumeOptions>)
                    .await
                {
                    Ok(())
                    | Err(DockerError::DockerResponseServerError {
                        status_code: 404, ..
                    }) => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contexts_carry_the_expected_entries() {
        let names = |bytes: Vec<u8>| -> Vec<String> {
            tar::Archive::new(&bytes[..])
                .entries()
                .unwrap()
                .map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(
            names(tar_context(DOCKERFILE, &RELEASE_FILES)),
            ["Dockerfile", "entrypoint.sh", "theuth-install.sh"]
        );
        assert_eq!(
            names(tar_context(DOCKERFILE_DEV, &[("t", b"ELF")])),
            ["Dockerfile", "t"]
        );
        assert_eq!(names(tar_file("t", b"ELF", 0o755)), ["t"]);
        assert!(
            DOCKERFILE.contains("minisign -Vq")
                && DOCKERFILE.contains("COPY entrypoint.sh")
                && DOCKERFILE.contains("COPY theuth-install.sh")
                && DOCKERFILE_DEV.contains("COPY t /usr/local/bin/t")
        );
    }
}
