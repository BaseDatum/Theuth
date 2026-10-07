# Deploying the engine

This guide walks through a production deployment of the engine on a single Linux host. It assumes a recent systemd, a user with sudo rights and about twenty minutes.

## Prerequisites

You need Rust 1.98 or newer, `just`, and a PostgreSQL 16 server reachable from the host. The engine stores its state in PostgreSQL and keeps nothing on local disk except logs.

Check the toolchain first:

```bash
cargo --version
just --version
psql --version
```

If any of those is missing, install it before continuing; the build step below does not try to be clever about missing tools.

## Install

Clone the repository and build a release binary. The build takes two to four minutes on a laptop and roughly a minute on a build server.

```bash
git clone https://github.com/example/engine.git
cd engine
just build-release
```

Copy the binary to `/usr/local/bin/engine` and the sample unit file to `/etc/systemd/system/engine.service`. The unit runs the engine as its own user, restarts it on failure and sends logs to the journal.

## Configure

Configuration lives in `/etc/engine/engine.toml`. The three keys most deployments change are the database URL, the listen address and the log level:

```toml
database_url = "postgres://engine:secret@localhost/engine"
listen = "127.0.0.1:8080"
log_level = "info"
```

Everything else has a sensible default. The full key list, with types and defaults, is in the reference section of the handbook.

## Run as a service

Enable and start the unit, then check that it answered its first health probe:

```bash
sudo systemctl enable --now engine
curl -fsS http://127.0.0.1:8080/healthz
```

A healthy instance prints `ok` and exits zero. A failing probe prints the first error from the journal, which is almost always a wrong database URL or a port already in use.

## Upgrade

Upgrades are a binary swap followed by a restart. Migrations run on start-up and are idempotent, so a rolled-back binary can start against a migrated database without harm.

1. Build the new release.
2. Copy it over the old binary.
3. Restart the unit and watch the health probe.

Keep the previous binary beside the new one for a day; rolling back is then a rename and a restart.

## Troubleshooting

The engine logs one line per request at `info` and a stack trace at `error`. When something looks wrong, raise the log level to `debug` for a few minutes rather than guessing. Common symptoms and their usual causes:

| symptom | cause |
|---|---|
| health probe times out | listen address bound to the wrong interface |
| migrations fail at start | database user lacks `CREATE` on the schema |
| slow first request | cold connection pool; expected once per restart |

Ask in the forum if none of these match, and include the first error line from the journal, your engine version and your PostgreSQL version.
