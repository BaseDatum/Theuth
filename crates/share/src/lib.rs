//! Public web sharing, the client plugin (design doc 210): dials the share edge on `theuth/share/1`, terminates
//! visitors' TLS with the mesh's name key, proxies to a loopback port or a directory, and owns the share store,
//! tools, RPC and consent. Wire types come from `share-proto`. Stub: later phases fill it.
