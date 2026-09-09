//! Compile `proto/agent.proto` (Cursor's `agent.v1`) with protox, a pure-Rust
//! protoc, so the build needs no system `protoc`.

fn main() {
    println!("cargo:rerun-if-changed=proto/agent.proto");
    let fds = protox::compile(["proto/agent.proto"], ["proto"]).expect("agent.proto parses");
    prost_build::Config::new()
        .compile_fds(fds)
        .expect("prost generates agent.v1");
}
