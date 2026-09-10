# Rust toolchain lives under rustup but homebrew's rustup does not install
# cargo/rustc proxies on PATH. Every recipe prepends the active toolchain's bin.
toolchain_bin := `dirname "$(rustup which cargo)"`
export PATH := toolchain_bin + ":" + env("PATH")

# Print the toolchain bin dir; `export PATH="$(just toolchain):$PATH"` in any shell.
toolchain:
    @echo {{toolchain_bin}}

# Ensure the stable toolchain from rust-toolchain.toml is installed.
setup:
    rustup show active-toolchain || rustup toolchain install stable
    @echo "cargo: $(rustup which cargo)"

build *ARGS:
    cargo build {{ARGS}}

check *ARGS:
    cargo check --workspace --all-targets {{ARGS}}

test *ARGS:
    cargo test --workspace {{ARGS}}

clippy:
    cargo clippy --workspace --all-targets -- -D warnings

fmt:
    cargo fmt --all

run *ARGS:
    cargo run -p wth -- {{ARGS}}

# Release build of `wth` copied into ~/.cargo/bin. Not `cargo install`: that
# re-resolves (index + git fetch every time) and rebuilds in a scratch dir.
install:
    cargo build --release -p wth
    install -m 755 target/release/wth "${CARGO_HOME:-$HOME/.cargo}/bin/wth"
    @echo "installed $(command -v wth)"

# Export TypeScript bindings for the RPC wire types.
ts:
    cargo test -p rpc --features ts --quiet export_bindings

# Refresh the bundled model catalog: models.dev api.json trimmed to the fields
# `crates/models` reads and to providers genai can route (design doc 02 §1).
models:
    curl -sL https://models.dev/api.json | jq 'with_entries(select(.key | IN("google","openrouter","togetherai","fireworks-ai","moonshotai","alibaba","alibaba-cn","amazon-bedrock","opencode-go","github-copilot","google-vertex","google-vertex-anthropic","ollama-cloud","anthropic","openai","groq","xai","deepseek","cohere","zai","nebius","minimax","aihubmix"))) | map_values({id,name,env,models: (.models | map_values({id,name,limit,cost,reasoning,tool_call} | del(.cost | nulls)))})' > crates/models/data/models.json
