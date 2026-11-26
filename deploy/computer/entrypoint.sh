#!/bin/sh
# Entrypoint of the engine image (design doc 224 §5). On every start:
#
#   1. as root (a Fly volume is root-owned): own /data and /workspace, drop to uid 1000;
#   2. as PID 1 (Kubernetes, `docker run` without --init): re-exec under tini, so `t`'s parent
#      is PID 1 — `platform::service::managed()` — and an applied update exits 75 for a restart;
#   3. put the newest signed `t` in $THEUTH_DATA_DIR/bin: the image's (verified at build time)
#      or the release manifest's (verified here with minisign against the baked key), whichever
#      is newer than what is there. The engine's own updater (crates/update) then replaces
#      /data/bin/t in place, which is writable by uid 1000, and the restart policy brings it up;
#   4. first start with THEUTH_NODE_NAME: write mesh.name / class into config.yaml;
#   5. until the engine is paired: try the one-time pairing ticket (`t mesh add`) in the
#      background — $THEUTH_DATA_DIR/pair-ticket, else $THEUTH_PAIR_TICKET_FILE, else
#      $THEUTH_PAIR_TICKET — re-reading it so a fresh ticket can be dropped in after an expiry;
#   6. exec /data/bin/t --headless.
#
# THEUTH_BOOT_UPDATE=0 skips the manifest check (the baked version is still installed).
set -eu

DATA="${THEUTH_DATA_DIR:-/data}"
BIN="$DATA/bin"
BAKED=/usr/local/lib/theuth
PUBKEY_FILE=/usr/local/share/theuth/pubkey.txt
MANIFEST="${THEUTH_MANIFEST:-https://s3.rwx.dev/theuth/releases/latest.json}"
MARKER="$DATA/.theuth-paired"

log() { printf 'theuth-entrypoint: %s\n' "$*" >&2; }

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

# `t --version` prints `t 2.31.2`.
version_of() { "$1" --version 2>/dev/null | awk 'NR == 1 { print $2 }'; }

# is_newer A B: B is a higher version than A (A may be empty: nothing installed).
is_newer() {
	[ -n "$2" ] || return 1
	[ -n "$1" ] || return 0
	[ "$1" != "$2" ] && [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -n 1)" = "$2" ]
}

major() { printf '%s' "${1%%.*}"; }

# commit STAGE: move helpers first and `t` last into $BIN (same filesystem: each a rename),
# the order crates/update uses, so a `t` that exists always has its helper.
commit() {
	for f in "$1"/*; do
		case "${f##*/}" in
		t) ;;
		theuth-*) chmod 755 "$f" && mv -f "$f" "$BIN/" ;;
		*) log "unexpected file ${f##*/} in the release; not installed" ;;
		esac
	done
	chmod 755 "$1/t" && mv -f "$1/t" "$BIN/t"
}

install_baked() {
	stage="$BIN/.stage.$$"
	rm -rf "$stage" && mkdir -p "$stage"
	cp "$BAKED"/* "$stage/"
	commit "$stage"
	rm -rf "$stage"
}

# install_release VERSION URL SIGNATURE: the tarball the manifest names, verified with minisign
# against the key baked into the image; its trusted comment must name the file, and the
# unpacked `t` must report VERSION. Any failure leaves $BIN as it was.
install_release() {
	v=$1 url=$2 sig=$3
	stage="$BIN/.stage.$$"
	rm -rf "$stage" && mkdir -p "$stage/x"
	file="${url##*/}"
	case "$file" in
	"theuth_${v}_linux-"*.tar.gz) ;;
	*)
		log "manifest $v names $file; not a linux tarball of that version"
		rm -rf "$stage"
		return 1
		;;
	esac
	pub="$(base64 -d <"$PUBKEY_FILE" | sed -n 2p)"
	if ! curl -fsSL --retry 3 --max-time 600 -o "$stage/$file" "$url"; then
		log "download $url failed"
		rm -rf "$stage"
		return 1
	fi
	printf '%s' "$sig" | base64 -d >"$stage/t.minisig"
	if ! out="$(minisign -V -P "$pub" -m "$stage/$file" -x "$stage/t.minisig" 2>&1)"; then
		log "signature check of $file failed: $out"
		rm -rf "$stage"
		return 1
	fi
	case "$out" in
	*"file:$file"*) ;;
	*)
		log "signature of $file is for another file: $out"
		rm -rf "$stage"
		return 1
		;;
	esac
	tar --no-same-owner -xzf "$stage/$file" -C "$stage/x"
	got="$(version_of "$stage/x/t")"
	if [ "$got" != "$v" ]; then
		log "the $v tarball holds t $got; not installed"
		rm -rf "$stage"
		return 1
	fi
	commit "$stage/x"
	rm -rf "$stage"
}

# manifest_entry: `VERSION URL SIGNATURE` for this machine's `cli-linux-<arch>`, from $MANIFEST.
manifest_entry() {
	case "$(uname -m)" in
	x86_64 | amd64) arch=x86_64 ;;
	aarch64 | arm64) arch=aarch64 ;;
	*) return 1 ;;
	esac
	curl -fsSL --retry 2 --max-time 30 "$MANIFEST" | python3 -c '
import json, sys
m = json.load(sys.stdin)
p = m["platforms"]["cli-linux-" + sys.argv[1]]
print(m["version"], p["url"], p["signature"])
' "$arch"
}

# 3.
mkdir -p "$BIN"
have="$(version_of "$BIN/t" || true)"
baked="$(version_of "$BAKED/t")"
if is_newer "$have" "$baked"; then
	log "installing the image's t $baked into $BIN (had ${have:-none})"
	install_baked
	have="$baked"
fi
if [ "${THEUTH_BOOT_UPDATE:-1}" != 0 ]; then
	if entry="$(manifest_entry 2>/dev/null)"; then
		# Three space-separated fields; none holds a space.
		latest="${entry%% *}"
		rest="${entry#* }"
		url="${rest%% *}"
		sig="${rest#* }"
		if is_newer "$have" "$latest" && [ "$(major "$latest")" = "$(major "$have")" ]; then
			log "installing t $latest from the release manifest (had $have)"
			if install_release "$latest" "$url" "$sig"; then
				have="$latest"
			fi
		fi
	else
		log "release manifest $MANIFEST unreachable; starting t $have"
	fi
fi

# 4.
if [ ! -e "$DATA/config.yaml" ] && [ -n "${THEUTH_NODE_NAME:-}" ]; then
	case "$THEUTH_NODE_NAME" in
	*[!a-z0-9-]* | -*) log "THEUTH_NODE_NAME must be [a-z0-9-]; mesh.name left to the hostname" ;;
	*) printf 'mesh:\n  name: %s\n  class: container\n  nearby: false\n' "$THEUTH_NODE_NAME" >"$DATA/config.yaml" ;;
	esac
fi

# 5.
ticket() {
	for f in "$DATA/pair-ticket" "${THEUTH_PAIR_TICKET_FILE:-/run/theuth/pair/ticket}"; do
		if [ -s "$f" ]; then
			head -n 1 "$f" | tr -d ' \r\n'
			return
		fi
	done
	printf '%s' "${THEUTH_PAIR_TICKET:-}"
}

join_mesh() {
	tried=""
	while [ ! -e "$MARKER" ]; do
		tk="$(ticket)"
		if [ -n "$tk" ] && [ "$tk" != "$tried" ]; then
			if out="$("$BIN/t" --data-dir "$DATA" mesh add "$tk" 2>&1)"; then
				: >"$MARKER"
				rm -f "$DATA/pair-ticket"
				log "joined the mesh"
				return 0
			fi
			case "$out" in
			*"invalid or expired"* | *"refused"* | *"not a mesh pairing string"* | *"own pairing string"*)
				tried="$tk"
				log "pairing ticket rejected: $(printf '%s' "$out" | tail -n 1); write a fresh one to $DATA/pair-ticket"
				;;
			esac
		fi
		sleep 10
	done
}

# Double fork: the helper's parent exits at once, so PID 1 (tini, docker-init, Fly's init)
# reaps it, not `t`.
if [ ! -e "$MARKER" ]; then
	(join_mesh &)
fi

# 6.
exec "$BIN/t" --headless --data-dir "$DATA" "$@"
