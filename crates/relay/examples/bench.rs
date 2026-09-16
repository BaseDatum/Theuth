//! Throwaway relay bench (design doc 31 §11): plays both ends against a
//! running `wth-relay` on loopback and histograms DATA echo RTT.
//!
//! ```text
//! cargo run --release -p relay --example bench -- ws://127.0.0.1:8787 <harnesses> <devices-per-harness> <msgs/s per device> <seconds> [relay-pid]
//! ```
//!
//! Each fake harness completes the NK registration with a generated static
//! key and echoes every `DATA` frame back unchanged; each device sends a
//! timestamped `DATA` at the given rate and records the round trip. Every
//! socket carries a distinct `X-Forwarded-For` so the relay's per-IP token
//! bucket does not throttle the driver. Prints p50/p99/max RTT, throughput
//! and the relay's RSS (`ps -o rss=`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use bytes::{BufMut, BytesMut};
use futures::{SinkExt, StreamExt};
use tokio::sync::{Semaphore, oneshot};
use tokio::task::JoinSet;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::{Message, http};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async_with_config};

type Ws = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

const NOISE: &str = "Noise_NK_25519_AESGCM_SHA256";
const HELLO: u8 = 0;
const DATA: u8 = 2;
const PRESENCE: u8 = 4;
const CONNECT_PARALLEL: usize = 256;

struct Args {
    url: String,
    harnesses: usize,
    devices: usize,
    rate: u32,
    seconds: u64,
    pid: Option<u32>,
}

fn args() -> Option<Args> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() < 5 {
        return None;
    }
    Some(Args {
        url: a[0].trim_end_matches('/').to_string(),
        harnesses: a[1].parse().ok()?,
        devices: a[2].parse().ok()?,
        rate: a[3].parse().ok()?,
        seconds: a[4].parse().ok()?,
        pid: a.get(5).and_then(|p| p.parse().ok()),
    })
}

fn request(url: &str, n: usize) -> http::Request<()> {
    let mut req = url.into_client_request().expect("url");
    req.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static("wth-link-1"),
    );
    let ip = format!("10.{}.{}.{}", (n >> 16) & 255, (n >> 8) & 255, n & 255);
    req.headers_mut()
        .insert("X-Forwarded-For", HeaderValue::from_str(&ip).expect("ip"));
    req
}

/// Connect with at most [`CONNECT_PARALLEL`] handshakes in flight; the permit is released once upgraded.
async fn connect(url: &str, n: usize, sem: &Semaphore) -> anyhow::Result<Ws> {
    let _p = sem.acquire().await?;
    Ok(connect_async_with_config(request(url, n), None, true)
        .await?
        .0)
}

async fn harness(
    url: String,
    n: usize,
    sem: Arc<Semaphore>,
    keys: snow::Keypair,
    ready: oneshot::Sender<()>,
) -> anyhow::Result<()> {
    let id = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&keys.public);
    let mut ws = connect(&format!("{url}/h/{id}?role=harness"), n, &sem).await?;
    let mut prologue = b"wth-link-1".to_vec();
    prologue.extend_from_slice(id.as_bytes());
    let mut hs = snow::Builder::new(NOISE.parse()?)
        .local_private_key(&keys.private)?
        .prologue(&prologue)?
        .build_responder()?;
    let Some(Ok(Message::Binary(msg1))) = ws.next().await else {
        anyhow::bail!("no HELLO")
    };
    anyhow::ensure!(msg1.len() > 5 && msg1[0] == HELLO, "bad HELLO");
    let mut buf = [0u8; 128];
    hs.read_message(&msg1[5..], &mut buf)?;
    let n = hs.write_message(&[], &mut buf)?;
    let mut f = BytesMut::with_capacity(5 + n);
    f.put_u8(HELLO);
    f.put_u32_le(0);
    f.put_slice(&buf[..n]);
    ws.send(Message::Binary(f.freeze())).await?;
    let _ = ready.send(());
    while let Some(msg) = ws.next().await {
        match msg? {
            // DATA conn payload → echo the identical frame.
            Message::Binary(b) if b.first() == Some(&DATA) => ws.send(Message::Binary(b)).await?,
            Message::Binary(_) => {}
            Message::Close(_) => break,
            _ => {}
        }
    }
    Ok(())
}

async fn device(
    url: String,
    n: usize,
    sem: Arc<Semaphore>,
    id: String,
    rate: u32,
    deadline: Instant,
) -> anyhow::Result<Vec<u64>> {
    let mut ws = connect(&format!("{url}/h/{id}"), n, &sem).await?;
    loop {
        match ws.next().await {
            Some(Ok(Message::Binary(b)))
                if b.first() == Some(&PRESENCE) && b.get(1) == Some(&1) =>
            {
                break;
            }
            Some(Ok(Message::Binary(_))) => {}
            Some(Ok(Message::Close(f))) => anyhow::bail!("closed before online: {f:?}"),
            Some(Ok(_)) => {}
            Some(Err(e)) => return Err(e.into()),
            None => anyhow::bail!("socket ended before online"),
        }
    }
    let start = Instant::now();
    // Random phase so devices do not fire in lockstep; a synchronized burst measures queueing, not the relay.
    let period = Duration::from_secs_f64(1.0 / f64::from(rate.max(1)));
    let phase = period.mul_f64((n % 1000) as f64 / 1000.0);
    let mut tick = tokio::time::interval_at((start + phase).into(), period);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut rtts =
        Vec::with_capacity((u64::from(rate) * (deadline - start).as_secs()) as usize + 16);
    let mut pad = [0u8; 40];
    loop {
        tokio::select! {
            _ = tick.tick() => {
                let t = start.elapsed().as_nanos() as u64;
                pad[..8].copy_from_slice(&t.to_le_bytes());
                let mut f = BytesMut::with_capacity(1 + pad.len());
                f.put_u8(DATA);
                f.put_slice(&pad);
                ws.send(Message::Binary(f.freeze())).await?;
            }
            msg = ws.next() => match msg {
                Some(Ok(Message::Binary(b))) if b.first() == Some(&DATA) && b.len() >= 9 => {
                    let sent = u64::from_le_bytes(b[1..9].try_into().expect("8 bytes"));
                    rtts.push((start.elapsed().as_nanos() as u64).saturating_sub(sent));
                }
                Some(Ok(Message::Binary(b))) => anyhow::bail!("unexpected op {}", b[0]),
                Some(Ok(Message::Close(f))) => anyhow::bail!("closed: {f:?}"),
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(e.into()),
                None => break,
            },
            _ = tokio::time::sleep_until(deadline.into()) => break,
        }
    }
    let _ = ws.close(None).await;
    Ok(rtts)
}

fn rss_kib(pid: Option<u32>) -> Option<u64> {
    let pid = pid.or_else(|| {
        let out = std::process::Command::new("pgrep")
            .args(["-x", "wth-relay"])
            .output()
            .ok()?;
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()?
            .trim()
            .parse()
            .ok()
    })?;
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let Some(a) = args() else {
        eprintln!(
            "usage: bench <ws://host:port> <harnesses> <devices-per-harness> <msgs/s-per-device> <seconds> [relay-pid]"
        );
        std::process::exit(2);
    };
    let builder = snow::Builder::new(NOISE.parse()?);
    let mut ids = Vec::with_capacity(a.harnesses);
    let mut readies = Vec::with_capacity(a.harnesses);
    let mut harnesses = JoinSet::new();
    let connects = Arc::new(Semaphore::new(CONNECT_PARALLEL));
    for h in 0..a.harnesses {
        let keys = builder.generate_keypair()?;
        ids.push(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&keys.public));
        let (tx, rx) = oneshot::channel();
        readies.push(rx);
        let (url, sem) = (a.url.clone(), connects.clone());
        harnesses.spawn(harness(url, h, sem, keys, tx));
    }
    for r in readies {
        r.await
            .map_err(|_| anyhow::anyhow!("a harness failed to register"))?;
    }
    eprintln!("{} harnesses registered", a.harnesses);

    let t0 = Instant::now();
    let deadline = t0 + Duration::from_secs(a.seconds);
    let mut devices = JoinSet::new();
    let mut n = a.harnesses;
    for id in &ids {
        for _ in 0..a.devices {
            n += 1;
            let (url, id, sem, rate) = (a.url.clone(), id.clone(), connects.clone(), a.rate);
            devices.spawn(device(url, n, sem, id, rate, deadline));
        }
    }
    let total_sockets = a.harnesses + a.harnesses * a.devices;

    let mut rtts: Vec<u64> = Vec::new();
    let mut failed = 0usize;
    while let Some(r) = devices.join_next().await {
        match r? {
            Ok(v) => rtts.extend(v),
            Err(e) => {
                failed += 1;
                if failed <= 5 {
                    eprintln!("device error: {e}");
                }
            }
        }
    }
    let elapsed = t0.elapsed().as_secs_f64();
    let rss = rss_kib(a.pid);
    harnesses.abort_all();

    rtts.sort_unstable();
    let pct = |p: f64| -> f64 {
        if rtts.is_empty() {
            return 0.0;
        }
        let i = ((rtts.len() - 1) as f64 * p).round() as usize;
        rtts[i] as f64 / 1_000.0
    };
    println!(
        "sockets        {total_sockets} ({} harness, {} device, {failed} device failures)",
        a.harnesses,
        a.harnesses * a.devices
    );
    println!(
        "echoes         {} in {elapsed:.1}s = {:.0}/s ({:.0} relay frames/s)",
        rtts.len(),
        rtts.len() as f64 / elapsed,
        2.0 * rtts.len() as f64 / elapsed
    );
    println!("rtt p50        {:.1} µs", pct(0.50));
    println!("rtt p99        {:.1} µs", pct(0.99));
    println!("rtt max        {:.1} µs", pct(1.0));
    match rss {
        Some(kib) => println!(
            "relay rss      {} MiB ({:.1} KiB/socket)",
            kib / 1024,
            kib as f64 / total_sockets.max(1) as f64
        ),
        None => println!("relay rss      unknown (pass the relay pid as the 6th argument)"),
    }
    Ok(())
}
