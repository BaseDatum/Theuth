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

# Export TypeScript bindings for the RPC wire types.
ts:
    cargo test -p rpc --features ts --quiet export_bindings
