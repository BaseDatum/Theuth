//! Against the microsandbox runtime (Apple Silicon or Linux+KVM):
//! `cargo test -p sandbox --test vm -- --ignored` creates `spike-s2-test`, exercises the
//! `VmRuntime` surface and destroys it. The non-ignored test only builds the runtime and runs
//! preflight on this host.
use std::{collections::BTreeMap, time::Duration};

use bytes::Bytes;
use sandbox::{
    Cmd, Progress,
    types::VmConfig,
    vm::{
        self, ImageRef, Init, Mount, NetPolicy, Port, ResizeStatus, ResourceChange, RuntimeState,
        VmHandle, VmRuntime, VmSpec, VmStatus,
    },
};

const IMAGE: &str = "ghcr.io/basedatum/theuth-vm-base:trixie";
const NAME: &str = "spike-s2-test";

#[tokio::test]
async fn runtime_builds_and_preflight_answers() {
    let rt = vm::runtime(&VmConfig::default()).expect("microsandbox runtime");
    assert_eq!(rt.kind(), "microsandbox");
    let p = rt.preflight().await;
    assert!(p.host.cores > 0);
    assert!(p.checked_at_ms > 0);
    // Every blocker is user-facing: a code and a sentence.
    for b in p.blockers.iter().chain(&p.warnings) {
        assert!(!b.code.is_empty() && !b.message.is_empty(), "{b:?}");
    }
    let bad = VmConfig {
        runtime: "firecracker".into(),
        ..VmConfig::default()
    };
    assert!(vm::runtime(&bad).is_err());
}

fn spec(share: &std::path::Path, ports: Vec<Port>) -> VmSpec {
    VmSpec {
        name: NAME.into(),
        image: ImageRef::Oci { r#ref: IMAGE.into() },
        cpus: 1,
        max_cpus: 2,
        memory_mib: 1024,
        max_memory_mib: 2048,
        root_disk_mib: 4096,
        mounts: vec![Mount {
            host: share.to_path_buf(),
            guest: "/shared/a".into(),
            readonly: false,
        }],
        ports,
        network: NetPolicy::PublicHost,
        init: Init::Auto,
        hostname: NAME.into(),
        env: BTreeMap::new(),
        files: vec![vm::PrebootFile {
            path: "/etc/preboot".into(),
            bytes: b"yes".to_vec(),
            mode: 0o644,
        }],
        labels: BTreeMap::new(),
        ephemeral: false,
        idle_timeout_secs: None,
        max_duration_secs: None,
    }
}

async fn sh(rt: &dyn VmRuntime, h: &VmHandle, line: &str) -> String {
    let o = rt.exec(h, Cmd::new(line)).await.unwrap();
    assert!(o.ok(), "`{line}` exit {}: {}", o.status, o.reason());
    o.stdout_str()
}

#[tokio::test]
#[ignore = "boots a real microVM; needs the microsandbox runtime installed"]
async fn vm_round_trip() {
    let rt = vm::runtime(&VmConfig::default()).unwrap();
    let out = Progress::stdout();
    let pf = rt.preflight().await;
    assert!(matches!(pf.runtime, RuntimeState::Installed { .. }), "{pf:?}");
    assert!(pf.blockers.is_empty(), "{:?}", pf.blockers);

    let h = VmHandle {
        name: NAME.into(),
        runtime_id: None,
    };
    let _ = rt.destroy(&h).await;
    let share = std::env::temp_dir().join("theuth-vm-test-share");
    std::fs::create_dir_all(&share).unwrap();
    std::fs::write(share.join("host.txt"), "host").unwrap();

    let ports = vm::microsandbox::resolve_ports(&[Port {
        host_ip: "127.0.0.1".into(),
        host: 0,
        guest: 7800,
        udp: true,
    }])
    .unwrap();
    assert_ne!(ports[0].host, 0);
    let spec = spec(&share, ports);

    let h = rt.create(&spec, &out).await.unwrap();
    assert_eq!(rt.status(&h).await.unwrap(), VmStatus::Running);
    assert_eq!(
        sh(rt.as_ref(), &h, "cat /proc/1/comm /etc/preboot /shared/a/host.txt").await,
        "systemd\nyeshost"
    );
    // Idempotent create adopts the running VM.
    let again = rt.create(&spec, &out).await.unwrap();
    assert_eq!(again.name, h.name);

    rt.put(&h, "/opt/x/run.sh", Bytes::from_static(b"#!/bin/sh\necho ran"), 0o755)
        .await
        .unwrap();
    assert_eq!(sh(rt.as_ref(), &h, "/opt/x/run.sh; stat -c %a /opt/x/run.sh").await, "ran\n755");

    let r = rt
        .resize(
            &h,
            ResourceChange {
                cpus: Some(2),
                root_disk_mib: Some(6144),
                max_memory_mib: Some(3072),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(r.applied.cpus, Some(2));
    assert_eq!(r.applied.root_disk_mib, Some(6144));
    assert_eq!(r.next_start.max_memory_mib, Some(3072));
    assert!(r.restart_required);
    assert!(matches!(r.status, ResizeStatus::Applied | ResizeStatus::Converging), "{r:?}");
    assert_eq!(sh(rt.as_ref(), &h, "nproc").await, "2");
    assert!(rt.resize(&h, ResourceChange { root_disk_mib: Some(1024), ..Default::default() }).await.is_err(), "disk shrink must be refused");

    let m = rt.metrics(&h).await.unwrap();
    assert!(m.memory_limit_bytes >= 1024 << 20 && m.uptime_secs < 3600, "{m:?}");
    assert!(!rt.logs(&h, 3).await.unwrap().is_empty());

    sh(rt.as_ref(), &h, "echo m1 > /root/marker").await;
    rt.stop(&h, Duration::from_secs(30)).await.unwrap();
    assert_eq!(rt.status(&h).await.unwrap(), VmStatus::Stopped);
    let share_b = std::env::temp_dir().join("theuth-vm-test-share-b");
    std::fs::create_dir_all(&share_b).unwrap();
    let mut mounts = spec.mounts.clone();
    mounts.push(Mount {
        host: share_b.clone(),
        guest: "/shared/b".into(),
        readonly: true,
    });
    let p = rt.set_mounts(&h, &mounts).await.unwrap();
    assert!(!p.restart_required);
    rt.start(&h, &out).await.unwrap();
    assert_eq!(
        sh(rt.as_ref(), &h, "cat /proc/1/comm /root/marker; ls -d /shared/b; nproc").await,
        "systemd\nm1\n/shared/b\n2"
    );

    let snap = rt.snapshot(&h, "clean", false, &out).await.unwrap();
    assert_eq!(snap.0, format!("{NAME}:clean"));
    assert!(rt.snapshot(&h, "full", true, &out).await.is_err());
    let snaps = rt.snapshots(&h).await.unwrap();
    assert!(snaps.iter().any(|s| s.label == "clean" && !s.auto), "{snaps:?}");
    assert!(snaps.iter().any(|s| s.auto), "reconfigure helper snapshot: {snaps:?}");
    assert!(rt.list().await.unwrap().iter().any(|x| x.name == NAME));

    rt.pause(&h).await.unwrap();
    assert_eq!(rt.status(&h).await.unwrap(), VmStatus::Paused);
    rt.resume(&h).await.unwrap();
    assert_eq!(rt.status(&h).await.unwrap(), VmStatus::Running);

    rt.destroy(&h).await.unwrap();
    assert_eq!(rt.status(&h).await.unwrap(), VmStatus::Gone);
    for s in snaps {
        rt.delete_snapshot(&s.id).await.unwrap();
    }
    let _ = std::fs::remove_dir_all(&share);
    let _ = std::fs::remove_dir_all(&share_b);
}
