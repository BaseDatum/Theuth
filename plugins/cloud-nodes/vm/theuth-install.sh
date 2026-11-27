#!/bin/sh
# Install, configure and pair a headless theuth engine whose data dir is $THEUTH_DATA_DIR (design
# doc 224 §5). One script for both ways a node runs:
#   - the engine image (deploy/computer/entrypoint.sh calls it before `exec t`), and
#   - a native VM install (DigitalOcean cloud-init; the systemd unit runs `install` as
#     ExecStartPre and `join` as its own unit).
#
#   theuth-install.sh install         newest signed `t` into $THEUTH_DATA_DIR/bin
#   theuth-install.sh config NAME KIND   first start: mesh.name / class into config.yaml
#   theuth-install.sh join            until paired: `t mesh add <ticket>` (blocks; exit 0 when paired)
#
# install and config also seal a fresh node's vault (design doc 225): `secrets.key_store: sealed`
# is appended to the config.yaml `config` wrote, once both a `t` with the sealed vault is installed
# and no vault exists yet. The engine then keeps the vault key in memory until the creating engine
# adds its device unlock and the master password over the mesh; `secrets.key` is never written.
# A `t` without the sealed vault (`t vault --help` fails) would refuse the setting and not start,
# so it is left out; that node's vault stays as its release makes it.
#
# install: $THEUTH_BAKED_DIR (the image's build-time-verified release; unset on a VM) when it is
# newer than what is installed, then — unless THEUTH_BOOT_UPDATE=0 — the release manifest's
# `cli-linux-<arch>` tarball when that is newer and the same major: downloaded, verified with
# minisign against $THEUTH_PUBKEY_FILE (Tauri's base64 of the minisign public key file), its trusted
# comment must name the file and the unpacked `t --version` must equal the manifest's version.
# Never a downgrade; any failure leaves the installed `t` as it was. Exit 1 only if no `t` is
# installed afterwards. The engine's own updater (crates/update) later replaces $BIN/t in place:
# the directory belongs to the engine's user.
#
# join: the ticket is $THEUTH_DATA_DIR/pair-ticket, else $THEUTH_PAIR_TICKET_FILE, else
# $THEUTH_PAIR_TICKET, re-read every 10 s; a rejected ticket is not retried until it changes, so a
# fresh one can be written to pair-ticket after an expiry. Joined: `.theuth-paired` is written and
# pair-ticket deleted.
set -eu

DATA="${THEUTH_DATA_DIR:-/data}"
BIN="$DATA/bin"
BAKED="${THEUTH_BAKED_DIR:-}"
PUBKEY_FILE="${THEUTH_PUBKEY_FILE:-/usr/local/share/theuth/pubkey.txt}"
MANIFEST="${THEUTH_MANIFEST:-https://s3.rwx.dev/theuth/releases/latest.json}"
MARKER="$DATA/.theuth-paired"

log() { printf 'theuth-install: %s\n' "$*" >&2; }

# `t --version` prints `t 2.31.2`.
version_of() { "$1" --version 2>/dev/null | awk 'NR == 1 { print $2 }'; }

# is_newer A B: B is a higher version than A (A may be empty: nothing installed).
is_newer() {
	[ -n "$2" ] || return 1
	[ -n "$1" ] || return 0
	[ "$1" != "$2" ] && [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -n 1)" = "$2" ]
}

major() { printf '%s' "${1%%.*}"; }

# commit STAGE: helpers first and `t` last into $BIN (same filesystem: each a rename), the order
# crates/update uses, so a `t` that exists always has its helper.
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

# install_release VERSION URL SIGNATURE (the manifest's entry). Any failure leaves $BIN as it was.
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
	if ! curl -fsSL --retry 3 --max-time 900 -o "$stage/$file" "$url"; then
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
		log "the $v tarball holds t ${got:-(does not run)}; not installed"
		rm -rf "$stage"
		return 1
	fi
	commit "$stage/x"
	rm -rf "$stage"
}

# manifest_entry: `VERSION URL SIGNATURE` for this machine's `cli-linux-<arch>`.
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

cmd_install() {
	mkdir -p "$BIN"
	have="$(version_of "$BIN/t" || true)"
	if [ -n "$BAKED" ]; then
		baked="$(version_of "$BAKED/t")"
		if is_newer "$have" "$baked"; then
			log "installing the image's t $baked into $BIN (had ${have:-none})"
			install_baked
			have="$baked"
		fi
	fi
	if [ "${THEUTH_BOOT_UPDATE:-1}" != 0 ]; then
		if entry="$(manifest_entry 2>/dev/null)"; then
			# Three space-separated fields; none holds a space.
			latest="${entry%% *}"
			rest="${entry#* }"
			url="${rest%% *}"
			sig="${rest#* }"
			if is_newer "$have" "$latest" && { [ -z "$have" ] || [ "$(major "$latest")" = "$(major "$have")" ]; }; then
				log "installing t $latest from the release manifest (had ${have:-none})"
				if install_release "$latest" "$url" "$sig"; then
					have="$latest"
				fi
			fi
		else
			log "release manifest $MANIFEST unreachable; keeping t ${have:-none}"
		fi
	fi
	if [ -z "$have" ]; then
		log "no t installed in $BIN"
		return 1
	fi
	seal_config
}

CONFIG_MARK='# written by theuth-install.sh config (a cloud node, design doc 224)'

# seal_config: see the header. Only a config.yaml `config` wrote, only before the vault exists.
seal_config() {
	[ -s "$DATA/config.yaml" ] && [ -x "$BIN/t" ] || return 0
	[ "$(head -n 1 "$DATA/config.yaml")" = "$CONFIG_MARK" ] || return 0
	if grep -q '^secrets:' "$DATA/config.yaml"; then
		return 0
	fi
	for f in secrets.key vault-keys.json secrets.sqlite; do
		[ ! -e "$DATA/$f" ] || return 0
	done
	if ! "$BIN/t" vault --help >/dev/null 2>&1; then
		log "this t has no sealed vault yet; vault key_store left to the release"
		return 0
	fi
	printf 'secrets:\n  key_store: sealed\n' >>"$DATA/config.yaml"
	log "vault: key_store sealed (no key file; the creating engine protects it)"
}

# config NAME KIND: only when there is no config.yaml yet (the engine's first run appends its own
# defaults below these lines).
cmd_config() {
	name=$1 class=$2
	[ ! -e "$DATA/config.yaml" ] || return 0
	case "$name" in
	'' | *[!a-z0-9-]* | -*) log "node name must be [a-z0-9-]; mesh.name left to the hostname" ;;
	*)
		printf '%s\nmesh:\n  name: %s\n  class: %s\n  nearby: false\n' "$CONFIG_MARK" "$name" "$class" >"$DATA/config.yaml"
		seal_config
		;;
	esac
}

ticket() {
	for f in "$DATA/pair-ticket" "${THEUTH_PAIR_TICKET_FILE:-/run/theuth/pair/ticket}"; do
		if [ -s "$f" ]; then
			head -n 1 "$f" | tr -d ' \r\n'
			return
		fi
	done
	printf '%s' "${THEUTH_PAIR_TICKET:-}"
}

cmd_join() {
	tried=""
	while [ ! -e "$MARKER" ]; do
		tk="$(ticket)"
		if [ -n "$tk" ] && [ "$tk" != "$tried" ] && [ -x "$BIN/t" ]; then
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

case "${1:-}" in
install) cmd_install ;;
config) cmd_config "${2:-}" "${3:-container}" ;;
join) cmd_join ;;
*)
	log "usage: $0 install | config NAME [KIND] | join"
	exit 2
	;;
esac
