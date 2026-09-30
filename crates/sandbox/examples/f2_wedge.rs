//! `cargo run -p sandbox --example f2_wedge` — smoke bug 2 (79-smoke.md): after one boot dies in
//! the runtime child (`build_microvm`), every later create in the same process fails with
//! `insert run: FOREIGN KEY constraint failed`. Sequence: create `f2-a` → create `f2-bad` with an
//! impossible `max_memory` (the smoke's trigger) → create `f2-c` → snapshot it → list. Leaves no
//! `f2-*` sandbox or snapshot behind.
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use sandbox::{
    Progress,
    types::VmConfig,
    vm::{self, ImageRef, Init, Mount, NetPolicy, ResourceChange, VmHandle, VmRuntime, VmSpec, VmStatus},
};

const IMAGE: &str = "theuth-vm:dev";

fn spec(name: &str, max_memory_mib: u32) -> VmSpec {
    VmSpec {
        name: name.into(),
        image: ImageRef::Oci { r#ref: IMAGE.into() },
        cpus: 1,
        max_cpus: 2,
        memory_mib: 1024,
        max_memory_mib,
        root_disk_mib: 8192,
        mounts: Vec::new(),
        ports: Vec::new(),
        network: NetPolicy::PublicHost,
        init: Init::Auto,
        hostname: name.into(),
        env: BTreeMap::new(),
        files: Vec::new(),
        labels: BTreeMap::new(),
        ephemeral: false,
        idle_timeout_secs: None,
        max_duration_secs: None,
    }
}

async fn cleanup(rt: &dyn VmRuntime) {
    for n in ["f2-a", "f2-bad", "f2-c"] {
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
    let rt = vm::runtime(&VmConfig::default())?;
    let host_mib = u32::try_from(rt.preflight().await.host.mem_bytes >> 20)?;
    cleanup(rt.as_ref()).await;

    let t = Instant::now();
    let a = rt.create(&spec("f2-a", 2048), &out).await?;
    println!("1 create f2-a ok in {} ms", t.elapsed().as_millis());

    // The smoke's sequence on f2-a: live resize, an impossible ceiling for the next start,
    // a share change, stop, start (= stop → disk snapshot → remove → boot from the snapshot).
    let r = rt
        .resize(&a, ResourceChange { cpus: Some(2), memory_mib: Some(1536), ..Default::default() })
        .await?;
    println!("2a resize live → {:?}", r.status);
    let r = rt
        .resize(&a, ResourceChange { max_memory_mib: Some(host_mib), ..Default::default() })
        .await?;
    println!("2b resize max_memory {host_mib} → restart_required {}", r.restart_required);
    let share = std::env::temp_dir().join("f2-share");
    std::fs::create_dir_all(&share)?;
    let p = rt
        .set_mounts(
            &a,
            &[Mount { host: share.clone(), guest: "/shared/f2".into(), readonly: false }],
        )
        .await?;
    println!("2c set_mounts → restart_required {}", p.restart_required);
    rt.stop(&a, Duration::from_secs(20)).await?;
    println!("2d stop ok");
    let t = Instant::now();
    let bad = rt.start(&a, &out).await;
    println!(
        "2e start f2-a (recreate with max_memory {host_mib} MiB) → {} in {} ms",
        match &bad {
            Ok(_) => "UNEXPECTEDLY OK".to_owned(),
            Err(e) => format!("{e:#}"),
        },
        t.elapsed().as_millis()
    );
    let _ = std::fs::remove_dir_all(&share);

    let t = Instant::now();
    let c = rt.create(&spec("f2-c", 2048), &out).await;
    println!(
        "3 create f2-c → {} in {} ms",
        match &c {
            Ok(_) => "ok".to_owned(),
            Err(e) => format!("FAILED {e:#}"),
        },
        t.elapsed().as_millis()
    );
    let mut result = c.as_ref().map(|_| ()).map_err(|e| anyhow::anyhow!("{e:#}"));

    if let Ok(c) = &c {
        let id = rt.snapshot(c, "f2-clean", false, &out).await?;
        let listed = rt.snapshots(c).await?;
        let seen = listed.iter().any(|s| s.id == id);
        println!("4 snapshot {} listed by runtime.snapshots: {seen} ({} total)", id.0, listed.len());
        let ls = std::process::Command::new("msb").args(["snapshot", "ls"]).output()?;
        let ls = String::from_utf8_lossy(&ls.stdout);
        println!("5 msb snapshot ls sees it: {}", ls.contains("f2-clean"));
        if !seen || !ls.contains("f2-clean") {
            result = Err(anyhow::anyhow!("snapshot not indexed"));
        }
    }
    let _ = a;
    cleanup(rt.as_ref()).await;
    println!("cleanup done");
    result
}
