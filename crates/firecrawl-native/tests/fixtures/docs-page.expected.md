# Deploying the engine

This page walks through a production deployment of the example engine: installing the binary, writing a minimal configuration, and running it under a service manager so it restarts on failure and starts at boot.

## Prerequisites

You need a 64-bit Linux or macOS host with at least 2 GB of memory, a user account that can write to `/etc/engine`, and outbound HTTPS access for the first start, when the engine fetches its model manifest. Windows is supported through WSL 2 with the same steps.

- Rust 1.80 or newer if you build from source; the release tarball needs no toolchain.
- An open TCP port for the HTTP API (the default is 8080).
- A writable data directory; the default is `~/.engine`.

## Install

Download the release archive for your platform, verify its checksum and unpack it onto your `PATH`. The archive contains one static binary and a sample configuration file.

```
curl -LO https://releases.example.test/engine/1.4.2/engine-linux-x86_64.tar.gz
sha256sum -c engine-linux-x86_64.tar.gz.sha256
tar xzf engine-linux-x86_64.tar.gz -C /usr/local/bin engine
engine --version
```

Building from source is the alternative when no archive matches your platform. The build takes about four minutes on a laptop.

## Configure

The engine reads one YAML file. Every key has a default, so the smallest useful file names only the listen address and the data directory. Secrets never go in this file; they are read from the environment or the vault.

```rust
use engine::Config;

fn main() -> anyhow::Result<()> {
    let cfg = Config::load("/etc/engine/config.yaml")?;
    assert_eq!(cfg.listen, "127.0.0.1:8080");
    println!("data dir: {}", cfg.data_dir.display());
    Ok(())
}
```

Run `engine check` after every edit: it parses the file, resolves the data directory and reports the effective value of every key without starting the server.

## Run as a service

On Linux, install the provided `engine.service` unit, enable it and start it. The unit runs the engine as its own user, restarts it on failure after five seconds, and limits open files to 65 536. On macOS the equivalent is a `launchd` agent that the installer writes for you.

Check the journal for the line *listening on 127.0.0.1:8080*; the first start takes longer because the model manifest is fetched and cached.

## Troubleshooting

If the service exits immediately, run the binary by hand with the same configuration to see the error on the terminal. The two common causes are a port already in use and a data directory the service user cannot write. Both are reported by `engine check`.

See [Configuration](https://docs-page.example.test/reference/config/) for every key and [Upgrading](https://docs-page.example.test/upgrade/) for moving between releases.
