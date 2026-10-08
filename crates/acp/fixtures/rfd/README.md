# RFD subagent fixtures — SYNTHETIC

Hand-written for 141 §6.1 from `agent-client-protocol-schema` 1.10.2
(`SubagentUpdate`, `StateUpdate`, `SessionMessageChunk`; test shapes in
`v1/client.rs` `test_subagent_*`). No shipped agent emitted RFD v1 at the time
(141 F11), so these are **not recordings**. Each file is one JSON-RPC
notification as the client would read it. Session ids: `sess_parent` is the
harness's own session, `sess_child_1` the announced child.
