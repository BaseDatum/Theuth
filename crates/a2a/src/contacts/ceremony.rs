//! 183 §7.4 (W-23): the contact ceremony on one `theuth/a2a/1` stream, JSON lines of
//! [`PairLine`]. B (the invitee, who dials) commits to its nonce first, A answers with its
//! nonce and its signed card, B reveals its nonce and its signed card; each side verifies the
//! other's card against the endpoint id QUIC proved, and both show the same 6-digit code.
//! Nothing but these frames flows until both humans pressed Accept; a Reject, a commit or
//! card that does not check, or [`CEREMONY_SECS`] ends it, and each failure counts toward
//! [`FAILED_MAX`] for the window. Both Accepts pin the other's endpoint id in `peers.yaml`
//! (TOFU) and the inviter's window closes.

use std::sync::Arc;
use std::time::Duration;

use a2a_types::AgentCard;
use data_encoding::BASE32_NOPAD;
use replica::ByteStream;
use sha2::Digest as _;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use uuid::Uuid;

use super::{
    CEREMONY_SECS, CODE_DOMAIN, CodeInputs, ContactEvent, InviteTicket, PairFrame, PairLine, Role,
    TICKET_PREFIX, card_machine, reach, verify_card,
};
use crate::Ctx;
use crate::server::IrohGate;

/// The other side must answer each handshake frame within this.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(30);
/// The `Reject` reason a side sends when its ceremony timed out, so the other reads `expired`.
pub const EXPIRED: &str = "expired";

/// 16 random bytes, base32.
pub fn nonce() -> String {
    BASE32_NOPAD.encode(&rand::random::<[u8; 16]>())
}

/// base32 `SHA256(nonce)`: the invitee's commitment.
pub fn commit(nonce: &str) -> String {
    BASE32_NOPAD.encode(&sha2::Sha256::digest(nonce.as_bytes()))
}

/// The 6-digit code of §7.4, shown `482 913`.
pub fn code(i: &CodeInputs) -> String {
    let (lo, hi) = if i.id_a <= i.id_b {
        (&i.id_a, &i.id_b)
    } else {
        (&i.id_b, &i.id_a)
    };
    let mut h = sha2::Sha256::new();
    for part in [CODE_DOMAIN, lo, hi, &i.nonce_a, &i.nonce_b] {
        h.update(part.as_bytes());
    }
    let d = h.finalize();
    let n = u32::from_be_bytes([d[0], d[1], d[2], d[3]]) % 1_000_000;
    format!("{:03} {:03}", n / 1000, n % 1000)
}

/// `theuth-contact:1.<base32 json>`.
pub fn encode_ticket(t: &InviteTicket) -> String {
    let json = serde_json::to_vec(t).unwrap_or_default();
    format!("{TICKET_PREFIX}{}", BASE32_NOPAD.encode(&json))
}

/// Parse a ticket string (whitespace and case tolerated: it may come from a QR or a paste).
pub fn decode_ticket(s: &str) -> Result<InviteTicket, String> {
    let body = s
        .trim()
        .strip_prefix(TICKET_PREFIX)
        .ok_or("not a theuth contact ticket")?;
    let bytes = BASE32_NOPAD
        .decode(body.trim().to_ascii_uppercase().as_bytes())
        .map_err(|_| "the ticket is damaged".to_owned())?;
    serde_json::from_slice(&bytes).map_err(|_| "the ticket is damaged".to_owned())
}

pub(crate) async fn write_frame(
    w: &mut (impl AsyncWrite + Unpin),
    pair: PairFrame,
) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(&PairLine { pair }).map_err(std::io::Error::other)?;
    line.push(b'\n');
    w.write_all(&line).await?;
    w.flush().await
}

fn parse(line: &[u8]) -> Result<PairFrame, String> {
    serde_json::from_slice::<PairLine>(line)
        .map(|l| l.pair)
        .map_err(|e| format!("not a ceremony frame: {e}"))
}

/// The frames of `r`, one per line, on a channel (reading a line is not cancel-safe).
fn frames(mut r: tokio::io::ReadHalf<Box<dyn ByteStream>>) -> mpsc::Receiver<PairFrame> {
    let (tx, rx) = mpsc::channel(4);
    tokio::spawn(async move {
        while let Ok(Ok(line)) = tokio::time::timeout(
            Duration::from_secs(CEREMONY_SECS + 30),
            reach::read_line(&mut r),
        )
        .await
        {
            let Ok(frame) = parse(&line) else { return };
            if tx.send(frame).await.is_err() {
                return;
            }
        }
    });
    rx
}

async fn next(rx: &mut mpsc::Receiver<PairFrame>) -> Result<PairFrame, String> {
    match tokio::time::timeout(STEP_TIMEOUT, rx.recv()).await {
        Ok(Some(PairFrame::Reject { reason })) => Err(format!("rejected: {reason}")),
        Ok(Some(f)) => Ok(f),
        Ok(None) => Err("the other side closed the ceremony".into()),
        Err(_) => Err("the other side did not answer".into()),
    }
}

/// Best effort after Remove or Block: tell `eid` it is no longer a contact, then close every
/// connection to it.
pub async fn revoke(net: Arc<mesh::net::Net>, eid: String) {
    if let Ok(Ok(mut s)) = tokio::time::timeout(STEP_TIMEOUT, net.a2a_stream(&eid, &[], None)).await
    {
        let _ = write_frame(&mut s, PairFrame::Revoke {}).await;
        let _ = s.shutdown().await;
    }
    net.close_a2a(&eid);
}

/// Inviter (A): a stream whose first line is a ceremony frame.
pub(crate) async fn serve(
    c: Arc<Ctx>,
    remote: String,
    first: Vec<u8>,
    stream: Box<dyn ByteStream>,
) -> anyhow::Result<()> {
    let (r, mut w) = tokio::io::split(stream);
    let frame = parse(&first).map_err(anyhow::Error::msg)?;
    let (name, machine, commitment, ticket) = match frame {
        PairFrame::Request {
            name,
            machine,
            commit,
            ticket,
        } => (name, machine, commit, ticket),
        PairFrame::Revoke {} => {
            tracing::info!(%remote, "a2a: a contact removed this engine");
            return Ok(());
        }
        other => anyhow::bail!("a ceremony must open with a request, not {other:?}"),
    };
    let window_gate =
        c.window_open() && matches!(c.gate(&remote), IrohGate::Pair | IrohGate::Contact { .. });
    if let Err(why) = window_gate
        .then_some(())
        .ok_or_else(|| "no Add contact window is open".to_owned())
        .and_then(|()| c.admit_request(&remote, ticket.as_deref()))
    {
        let _ = write_frame(
            &mut w,
            PairFrame::Reject {
                reason: why.clone(),
            },
        )
        .await;
        anyhow::bail!(why);
    }
    let mut rx = frames(r);
    let mine = c.my_nonce_and_card();
    let outcome = async {
        let (nonce_a, card_a) = mine?;
        write_frame(
            &mut w,
            PairFrame::Challenge {
                nonce: nonce_a.clone(),
                card: Box::new(card_a),
            },
        )
        .await
        .map_err(|e| e.to_string())?;
        let PairFrame::Reveal { nonce, card } = next(&mut rx).await? else {
            return Err("expected the reveal".to_owned());
        };
        if commit(&nonce) != commitment {
            return Err("the revealed nonce does not match its commitment".into());
        }
        verify_card(&card, &remote)?;
        let me = c.me()?;
        Ok((
            code(&CodeInputs {
                id_a: me,
                id_b: remote.clone(),
                nonce_a,
                nonce_b: nonce,
            }),
            card,
        ))
    }
    .await;
    match outcome {
        Ok((code, card)) => {
            let (id, answers) =
                c.ceremony_started(Role::Inviter, &remote, &name, &machine, &card, code);
            exchange(c, id, card, answers, rx, w).await;
            Ok(())
        }
        Err(why) => {
            let _ = write_frame(
                &mut w,
                PairFrame::Reject {
                    reason: why.clone(),
                },
            )
            .await;
            c.ceremony_failed(&remote, Role::Inviter);
            anyhow::bail!(why)
        }
    }
}

/// Invitee (B): dial the inviter and run the handshake up to the code; the rest (both
/// Accepts) runs in the background. Returns the ceremony as `contact_pair` answers it.
pub(crate) async fn start(
    c: Arc<Ctx>,
    eid: String,
    hints: reach::Hints,
    ticket: Option<String>,
) -> Result<mesh::PairCeremony, String> {
    let net = c.net().ok_or("this engine has no mesh endpoint")?;
    let stream = net
        .a2a_stream(&eid, &hints.addrs, hints.relay.as_deref())
        .await
        .map_err(|e| format!("could not reach the inviter: {e:#}"))?;
    let (r, mut w) = tokio::io::split(stream);
    let mut rx = frames(r);
    let (nonce_b, card_b) = c.my_nonce_and_card()?;
    let (name, machine) = c.my_names();
    let io = |e: std::io::Error| e.to_string();
    write_frame(
        &mut w,
        PairFrame::Request {
            name,
            machine,
            commit: commit(&nonce_b),
            ticket,
        },
    )
    .await
    .map_err(io)?;
    let checked = async {
        let PairFrame::Challenge { nonce, card } = next(&mut rx).await? else {
            return Err("expected the challenge".to_owned());
        };
        verify_card(&card, &eid)?;
        Ok((nonce, card))
    }
    .await;
    let (nonce_a, card_a) = match checked {
        Ok(x) => x,
        Err(why) => {
            let _ = write_frame(
                &mut w,
                PairFrame::Reject {
                    reason: why.clone(),
                },
            )
            .await;
            return Err(why);
        }
    };
    write_frame(
        &mut w,
        PairFrame::Reveal {
            nonce: nonce_b.clone(),
            card: Box::new(card_b),
        },
    )
    .await
    .map_err(io)?;
    let code = code(&CodeInputs {
        id_a: eid.clone(),
        id_b: c.me()?,
        nonce_a,
        nonce_b,
    });
    let (their_name, their_machine) = (card_a.name.clone(), card_machine(&card_a).0);
    let (id, answers) = c.ceremony_started(
        Role::Invitee,
        &eid,
        &their_name,
        &their_machine,
        &card_a,
        code,
    );
    let view = c.ceremony_view(id).ok_or("the ceremony ended at once")?;
    tokio::spawn(exchange(c, id, card_a, answers, rx, w));
    Ok(view)
}

/// Both sides after the code: wait for this side's answer and the other's `Accept`, then pin;
/// a Reject from either side, or the deadline, ends it.
async fn exchange(
    c: Arc<Ctx>,
    id: Uuid,
    card: Box<AgentCard>,
    mut answers: mpsc::UnboundedReceiver<bool>,
    mut rx: mpsc::Receiver<PairFrame>,
    mut w: tokio::io::WriteHalf<Box<dyn ByteStream>>,
) {
    let deadline = tokio::time::sleep(Duration::from_secs(c.ceremony_secs()));
    tokio::pin!(deadline);
    let (mut mine, mut theirs) = (false, false);
    let end = loop {
        if mine && theirs {
            break match c.pin(id, &card).await {
                Ok(()) => "paired",
                Err(e) => {
                    tracing::warn!(ceremony = %id, "a2a: pin contact: {e}");
                    "rejected"
                }
            };
        }
        tokio::select! {
            () = &mut deadline => {
                let _ = write_frame(&mut w, PairFrame::Reject { reason: EXPIRED.into() }).await;
                break "expired";
            }
            a = answers.recv() => match a {
                Some(true) if !mine => {
                    mine = true;
                    c.ceremony_state(id, "accepted").await;
                    if write_frame(&mut w, PairFrame::Accept { exports: Vec::new() }).await.is_err() {
                        break "rejected";
                    }
                }
                Some(true) => {}
                Some(false) | None => {
                    let _ = write_frame(&mut w, PairFrame::Reject { reason: "rejected".into() }).await;
                    break "rejected";
                }
            },
            f = rx.recv() => match f {
                Some(PairFrame::Accept { .. }) => theirs = true,
                Some(PairFrame::Reject { reason }) if reason == EXPIRED => break "expired",
                Some(_) | None => break "rejected",
            },
        }
    };
    let _ = w.shutdown().await;
    if end != "paired" {
        let who = c
            .contacts
            .lock()
            .ceremonies
            .get(&id)
            .map(|r| (r.view.peer.0.clone(), r.role));
        if let Some((remote, role)) = who {
            c.ceremony_failed(&remote, role);
        }
    }
    c.ceremony_ended(id, end).await;
}

/// The invite ticket `t` as a dial target (expired tickets are refused before dialing).
/// 183 Z3-R2 (SSRF): private, link-local and loopback addresses in it are dropped, and such
/// a relay refuses the ticket, unless a LAN sighting carried them ([`super::dial_ok`]).
pub(crate) fn ticket_target(
    t: &InviteTicket,
    lan: &[std::net::IpAddr],
    private_ok: bool,
) -> Result<reach::Hints, String> {
    if t.exp <= mesh::identity::now_ms() {
        return Err("the invite ticket expired".into());
    }
    if let Some(relay) = &t.relay {
        let local = match url::Url::parse(relay)
            .map_err(|e| format!("the ticket's relay: {e}"))?
            .host()
        {
            Some(url::Host::Ipv4(i)) => !super::dial_ok(i.into(), lan, private_ok),
            Some(url::Host::Ipv6(i)) => !super::dial_ok(i.into(), lan, private_ok),
            Some(url::Host::Domain(d)) => !private_ok && d.eq_ignore_ascii_case("localhost"),
            None => true,
        };
        if local {
            return Err("the invite ticket's relay is a private address".into());
        }
    }
    Ok(reach::Hints {
        addrs: t
            .addrs
            .iter()
            .filter_map(|a| a.parse::<std::net::SocketAddr>().ok())
            .filter(|a| super::dial_ok(a.ip(), lan, private_ok))
            .collect(),
        relay: t.relay.clone(),
    })
}

/// Publish a ceremony's view.
pub(crate) async fn publish(c: &Ctx, view: mesh::PairCeremony) {
    c.rt.publish(ContactEvent::Pair(view)).await;
}
