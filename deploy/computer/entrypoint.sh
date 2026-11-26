#!/bin/sh
# Entrypoint of the engine image (design doc 224 §5). On every start:
#
#   1. as root (a Fly volume is root-owned): own /data and /workspace, drop to uid 1000;
#   2. as PID 1 (Kubernetes, `docker run` without --init): re-exec under tini, so `t`'s parent
#      is PID 1 — `platform::service::managed()` — and an applied update exits 75 for a restart;
#   3. `theuth-install.sh install`: the newest signed `t` (the image's, or the release
#      manifest's) into $THEUTH_DATA_DIR/bin, which uid 1000 owns, so the engine's own updater
#      replaces it in place and the restart policy brings the new one up;
#   4. `theuth-install.sh config`: first start with THEUTH_NODE_NAME → mesh.name, class container;
#   5. `theuth-install.sh join` in the background until the engine is paired;
#   6. exec $THEUTH_DATA_DIR/bin/t --headless.
#
# The same theuth-install.sh runs a native VM node (DigitalOcean) under systemd.
set -eu

DATA="${THEUTH_DATA_DIR:-/data}"
INSTALL=/usr/local/lib/theuth-install.sh

# 1.
if [ "$(id -u)" = 0 ]; then
	for d in "$DATA" /workspace; do
		if [ -d "$d" ] && [ "$(stat -c %u "$d")" != 1000 ]; then
			chown 1000:1000 "$d"
		fi
	done
	exec setpriv --reuid=1000 --regid=1000 --init-groups --inh-caps=-all "$0" "$@"
fi

# 2.
if [ "$$" = 1 ]; then
	exec tini -g -- "$0" "$@"
fi

# 3. The engine starts on whatever is installed even if the manifest step failed.
THEUTH_BAKED_DIR=/usr/local/lib/theuth "$INSTALL" install
# 4.
[ -z "${THEUTH_NODE_NAME:-}" ] || "$INSTALL" config "$THEUTH_NODE_NAME" container

# 5. Double fork: the helper's parent exits at once, so PID 1 (tini, docker-init, Fly's init)
# reaps it, not `t`.
if [ ! -e "$DATA/.theuth-paired" ]; then
	("$INSTALL" join &)
fi

# 6.
exec "$DATA/bin/t" --headless --data-dir "$DATA" "$@"
