//! `cargo run -p sandbox --example vm_spike` — exercises `VmRuntime` on the real runtime
//! (design 79 §14 step 1; S2 brief): preflight → install → create with TCP + UDP ports and a
//! bind mount → exec → live resize → disk grow → set_mounts on a stopped VM → start → UDP echo
//! through the published port → snapshot → restore → destroy. Prints a table of timings.
//! Leaves the runtime installed and no `spike-*` sandbox behind.
use std::{
    collections::BTreeMap,
    net::UdpSocket,
    path::Path,
    time::{Duration, Instant},
};

use bytes::Bytes;
use sandbox::{
    Cmd, Progress,
    types::VmConfig,
    vm::{
        self, ImageRef, Init, Mount, NetPolicy, Port, ResourceChange, RuntimeState, SnapshotId,
        VmHandle, VmRuntime, VmSpec, VmStatus,
    },
};

const IMAGE: &str = "ghcr.io/basedatum/theuth-vm-base:trixie";
const NAME: &str = "spike-s2";
const RESTORED: &str = "spike-s2-r";

struct Table(Vec<(String, u128, String)>);

impl Table {
    fn row(&mut self, step: &str, t: Instant, outcome: impl Into<String>) {
        let o = outcome.into();
        println!("  {step:<28} {:>7} ms  {o}", t.elapsed().as_millis());
        self.0.push((step.into(), t.elapsed().as_millis(), o));
    }
    fn print(&self) {
        println!("\n| step | ms | outcome |\n|---|---:|---|");
        for (s, ms, o) in &self.0 {
            println!("| {s} | {ms} | {o} |");
        }
    }
}

fn make_spec(name: &str, share: &Path, ports: Vec<Port>) -> VmSpec {
    VmSpec {
        name: name.into(),
        image: ImageRef::Oci {
            r#ref: IMAGE.into(),
        },
        cpus: 2,
        max_cpus: 4,
        memory_mib: 2048,
        max_memory_mib: 4096,
        root_disk_mib: 8192,
        mounts: vec![Mount {
            host: share.to_path_buf(),
            guest: "/shared/a".into(),
            readonly: false,
        }],
        ports,
        network: NetPolicy::PublicHost,
        init: Init::Auto,
        hostname: name.into(),
        env: BTreeMap::new(),
        files: vec![vm::PrebootFile {
            path: "/etc/theuth-preboot".into(),
            bytes: b"preboot\n".to_vec(),
            mode: 0o644,
        }],
        labels: BTreeMap::from([("io.theuth.spike".to_owned(), "s2".to_owned())]),
        ephemeral: false,
        idle_timeout_secs: None,
        max_duration_secs: None,
    }
}

fn udp_echo(port: u16) -> anyhow::Result<f64> {
    let s = UdpSocket::bind("127.0.0.1:0")?;
    s.set_read_timeout(Some(Duration::from_secs(3)))?;
    let mut buf = [0u8; 256];
    let mut total = 0.0;
    for i in 0..20u8 {
        let msg = [b'p', i];
        let t = Instant::now();
        s.send_to(&msg, ("127.0.0.1", port))?;
        let (n, _) = s.recv_from(&mut buf)?;
        anyhow::ensure!(buf[..n] == msg, "udp echo payload mismatch");
        total += t.elapsed().as_secs_f64() * 1000.0;
    }
    Ok(total / 20.0)
}

async fn sh(rt: &dyn VmRuntime, h: &VmHandle, line: &str) -> anyhow::Result<String> {
    let o = rt.exec(h, Cmd::new(line)).await?;
    anyhow::ensure!(o.ok(), "`{line}` exit {}: {}", o.status, o.reason());
    Ok(o.stdout_str())
}

async fn cleanup(rt: &dyn VmRuntime) {
    for n in [NAME, RESTORED] {
        let h = VmHandle {
            name: n.into(),
            runtime_id: None,
        };
        if rt.status(&h).await.ok() != Some(VmStatus::Gone) {
            let _ = rt.destroy(&h).await;
        }
        for s in rt.snapshots(&h).await.unwrap_or_default() {
            let _ = rt.delete_snapshot(&s.id).await;
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let out = Progress::stdout();
    let cfg = VmConfig::default();
    let rt = vm::runtime(&cfg)?;
    let mut tbl = Table(Vec::new());
    let run = Instant::now();

    let t = Instant::now();
    let pf = rt.preflight().await;
    tbl.row(
        "preflight",
        t,
        format!(
            "{:?} virt={:?} blockers={:?} warnings={:?}",
            pf.runtime,
            pf.virt,
            pf.blockers.iter().map(|b| &b.code).collect::<Vec<_>>(),
            pf.warnings.iter().map(|b| &b.code).collect::<Vec<_>>()
        ),
    );
    if !matches!(pf.runtime, RuntimeState::Installed { .. }) {
        let t = Instant::now();
        rt.install(&out).await?;
        tbl.row("install", t, format!("{:?}", rt.preflight().await.runtime));
    }
    if pf.blockers.iter().any(|b| !b.installable) {
        tbl.print();
        anyhow::bail!("blocked: {:?}", pf.blockers);
    }

    cleanup(rt.as_ref()).await;
    let share = std::env::temp_dir().join("theuth-spike-s2-share");
    std::fs::create_dir_all(&share)?;
    std::fs::write(share.join("from-host.txt"), "hi from host\n")?;
    let share_b = std::env::temp_dir().join("theuth-spike-s2-share-b");
    std::fs::create_dir_all(&share_b)?;

    let result = scenario(rt.as_ref(), &out, &share, &share_b, &mut tbl).await;
    let t = Instant::now();
    cleanup(rt.as_ref()).await;
    tbl.row("destroy both + snapshots", t, "ok");
    let _ = std::fs::remove_dir_all(&share);
    let _ = std::fs::remove_dir_all(&share_b);
    tbl.row("total", run, "");
    tbl.print();
    result
}

async fn scenario(
    rt: &dyn VmRuntime,
    out: &Progress,
    share: &Path,
    share_b: &Path,
    tbl: &mut Table,
) -> anyhow::Result<()> {
    let ports = vm::microsandbox::resolve_ports(&[
        Port {
            host_ip: "127.0.0.1".into(),
            host: 0,
            guest: 6080,
            udp: false,
        },
        Port {
            host_ip: "127.0.0.1".into(),
            host: 0,
            guest: 7800,
            udp: true,
        },
    ])?;
    let udp_port = ports[1].host;
    let spec = make_spec(NAME, share, ports.clone());

    let t = Instant::now();
    let h = rt.create(&spec, out).await?;
    tbl.row("create (layers cached?)", t, format!("{:?}", h));

    let t = Instant::now();
    let facts = sh(
        rt,
        &h,
        "cat /proc/1/comm; nproc; awk '/MemTotal/{print int($2/1024)}' /proc/meminfo; df -m / | awk 'NR==2{print $2}'; cat /etc/theuth-preboot; cat /shared/a/from-host.txt",
    )
    .await?;
    tbl.row("exec facts", t, facts.replace('\n', " · "));

    let t = Instant::now();
    rt.put(
        &h,
        "/root/bin/hello.sh",
        Bytes::from_static(b"#!/bin/sh\necho put-ok\n"),
        0o755,
    )
    .await?;
    let ok = sh(rt, &h, "/root/bin/hello.sh; stat -c %a /root/bin/hello.sh").await?;
    tbl.row("put + exec", t, ok.replace('\n', " · "));

    let t = Instant::now();
    let r = rt
        .resize(
            &h,
            ResourceChange {
                cpus: Some(4),
                memory_mib: Some(3072),
                ..Default::default()
            },
        )
        .await?;
    let g = sh(
        rt,
        &h,
        "nproc; awk '/MemTotal/{print int($2/1024)}' /proc/meminfo",
    )
    .await?;
    tbl.row(
        "resize live 2→4 cpu, 2→3 GiB",
        t,
        format!(
            "{:?} restart_required={} guest={}",
            r.status,
            r.restart_required,
            g.replace('\n', "/")
        ),
    );

    let t = Instant::now();
    let r = rt
        .resize(
            &h,
            ResourceChange {
                root_disk_mib: Some(12288),
                ..Default::default()
            },
        )
        .await?;
    let g = sh(rt, &h, "df -m / | awk 'NR==2{print $2}'").await?;
    tbl.row(
        "disk grow 8→12 GiB",
        t,
        format!("{:?} df={g} MiB", r.status),
    );

    let t = Instant::now();
    let r = rt
        .resize(
            &h,
            ResourceChange {
                max_memory_mib: Some(8192),
                ..Default::default()
            },
        )
        .await?;
    tbl.row(
        "resize max_memory 4→8 GiB",
        t,
        format!(
            "next_start={:?} restart_required={}",
            r.next_start, r.restart_required
        ),
    );

    let t = Instant::now();
    let m = rt.metrics(&h).await?;
    tbl.row(
        "metrics",
        t,
        format!(
            "cpu={:.1}% mem={}MiB/{}MiB disk_used={}MiB uptime={}s",
            m.cpu_percent,
            m.memory_bytes >> 20,
            m.memory_limit_bytes >> 20,
            m.disk_used_bytes >> 20,
            m.uptime_secs
        ),
    );
    let t = Instant::now();
    let logs = rt.logs(&h, 5).await?;
    tbl.row(
        "logs tail 5",
        t,
        format!(
            "{} lines, last source={:?}",
            logs.len(),
            logs.last().map(|l| &l.source)
        ),
    );

    sh(rt, &h, "echo marker-1 > /root/marker").await?;
    let t = Instant::now();
    rt.stop(&h, Duration::from_secs(30)).await?;
    tbl.row("stop", t, format!("{:?}", rt.status(&h).await?));

    let t = Instant::now();
    let mut mounts = spec.mounts.clone();
    mounts.push(Mount {
        host: share_b.to_path_buf(),
        guest: "/shared/b".into(),
        readonly: true,
    });
    let p = rt.set_mounts(&h, &mounts).await?;
    tbl.row("set_mounts (stopped)", t, format!("{p:?}"));

    let t = Instant::now();
    rt.start(&h, out).await?;
    let g = sh(
        rt,
        &h,
        "cat /proc/1/comm; cat /root/marker; ls -d /shared/a /shared/b; nproc; df -m / | awk 'NR==2{print $2}'; cat /etc/theuth-preboot",
    )
    .await?;
    tbl.row("start (recreate w/ mounts)", t, g.replace('\n', " · "));

    let t = Instant::now();
    sh(
        rt,
        &h,
        "nohup socat -T2 UDP4-LISTEN:7800,fork,reuseaddr EXEC:cat >/dev/null 2>&1 & sleep 0.3; echo started",
    )
    .await
    .ok();
    let echo = match udp_echo(udp_port) {
        Ok(ms) => format!("ok, mean rtt {ms:.2} ms (host 127.0.0.1:{udp_port} → guest 7800)"),
        Err(e) => {
            // theuth-vm-base:trixie has no socat; a Python fallback is available after apt.
            let _ = sh(rt, &h, "command -v python3 || (apt-get update -qq && apt-get install -y -qq python3) >/dev/null 2>&1").await;
            let _ = sh(rt, &h, "nohup python3 -c 'import socket;s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM);s.bind((\"0.0.0.0\",7800))\nwhile 1:\n d,a=s.recvfrom(65535);s.sendto(d,a)' >/dev/null 2>&1 & sleep 0.5; echo started").await;
            match udp_echo(udp_port) {
                Ok(ms) => format!(
                    "ok via python3, mean rtt {ms:.2} ms (host 127.0.0.1:{udp_port} → guest 7800)"
                ),
                Err(e2) => format!("FAILED socat: {e}; python: {e2}"),
            }
        }
    };
    tbl.row("udp echo (published port)", t, echo);

    let t = Instant::now();
    let snap = rt.snapshot(&h, "clean", false, out).await?;
    tbl.row("snapshot disk (running)", t, snap.0.clone());
    let t = Instant::now();
    let full = rt.snapshot(&h, "full", true, out).await;
    tbl.row(
        "snapshot full (expected refusal)",
        t,
        match full {
            Ok(id) => format!("UNEXPECTED ok {id:?}"),
            Err(e) => format!("refused: {e}"),
        },
    );

    let t = Instant::now();
    let rspec = make_spec(
        RESTORED,
        share,
        vm::microsandbox::resolve_ports(&spec.ports)?,
    );
    let rh = rt
        .restore(&SnapshotId(snap.0.clone()), RESTORED, &rspec, out)
        .await?;
    let g = sh(
        rt,
        &rh,
        "cat /proc/1/comm; hostname; cat /root/marker; nproc; ls -d /shared/a",
    )
    .await?;
    tbl.row("restore as spike-s2-r", t, g.replace('\n', " · "));

    let t = Instant::now();
    let snaps = rt.snapshots(&h).await?;
    let listed = rt.list().await?;
    tbl.row(
        "snapshots + list",
        t,
        format!(
            "snapshots={:?} list={:?}",
            snaps
                .iter()
                .map(|s| format!("{}{}", s.label, if s.auto { "(auto)" } else { "" }))
                .collect::<Vec<_>>(),
            listed.iter().map(|h| &h.name).collect::<Vec<_>>()
        ),
    );

    let t = Instant::now();
    rt.pause(&h).await?;
    let paused = rt.status(&h).await?;
    rt.resume(&h).await?;
    tbl.row(
        "pause/resume",
        t,
        format!("{paused:?} → {:?}", rt.status(&h).await?),
    );
    Ok(())
}
