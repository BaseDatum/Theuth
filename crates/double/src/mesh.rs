//! Training bundles over the mesh (126 §8.4): `theuth/double/1`. Modelled on
//! `artifact::mesh` (crates/artifact/src/mesh.rs): `LengthDelimitedCodec`
//! frames of postcard on an iroh QUIC stream opened with
//! `Transport::open(peer, ALPN_DOUBLE)` and accepted with
//! `transport.accept(ALPN_DOUBLE)`. The constant lives in `replica` with every
//! other ALPN ([`replica::ALPN_DOUBLE`]) and is registered in the endpoint's
//! stream list (crates/mesh/src/net.rs `STREAM_ALPNS`).
//!
//! Wire (W2): one `TrainBundle { run_id, manifest, sha }` request per stream,
//! the bundle in chunks, then `TrainResult { run_id, adapter, head, manifest,
//! metrics }` streamed back the same way. The bundle never carries weights;
//! the node fetches the base checkpoint itself. A node keeps nothing after the
//! result is acknowledged.

pub use replica::ALPN_DOUBLE;

// 126 W2: `pub async fn serve(transport, store, …)` — the accept loop that
// runs a bundle as a `jobs`-style trainer process under build-lock, and
// `pub async fn train_on(peer, bundle) -> TrainResult` — the laptop side.
// Registered from `DoublePlugin::start` once the mesh transport is up, the
// way `artifact::mesh` is started; venue choice is `venue.rs`.
