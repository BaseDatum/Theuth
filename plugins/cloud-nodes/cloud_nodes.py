"""Hosted theuth nodes (design doc 224): a remote engine container on a provider the user already
has an account with, joined to the owner's mesh with a one-time ticket, its state on a volume.

Providers: DigitalOcean (Droplet + Volume + Cloud Firewall, REST v2), Fly.io (App + Volume +
Machine, Machines API), Kubernetes (`t mesh add --k8s`, which renders the kustomize base embedded
in `t`), AWS (credentials are discovered; create/destroy are not implemented — the plan is in 224
§10). Everything this module makes carries `theuth-managed` and `theuth-node:<name>` (labels,
tags or metadata), and `list()` finds it by them.

Credentials, first match wins: the plugin's environment (`plugin.yaml` `env:` can fill it from the
theuth vault with `{{secret:…}}`), then the provider CLI's own config file. Values are never
returned, logged or put on a command line.

No SDK import here: `main.py` wires these functions to tools; tests drive them with a fake HTTP
transport and a fake `t`.
"""

import base64
import hashlib
import json
import os
import re
import shutil
import subprocess
import time
import urllib.error
import urllib.request
from pathlib import Path

TAG_MANAGED = "theuth-managed"
ENGINE_IMAGE = "ghcr.io/basedatum/theuth/engine:latest"
MANIFEST = "https://s3.rwx.dev/theuth/releases/latest.json"
NAME_RE = re.compile(r"^[a-z]([a-z0-9-]{0,38}[a-z0-9])?$")
TICKET_RE = re.compile(r"^1\.[A-Za-z2-7]+$")

DO_API = "https://api.digitalocean.com/v2"
FLY_API = "https://api.machines.dev/v1"

# List prices in USD per month, 2026; estimates for the dry run, not a bill.
DO_SIZES = {
    "s-1vcpu-1gb": 6.0, "s-1vcpu-2gb": 12.0, "s-2vcpu-2gb": 18.0, "s-2vcpu-4gb": 24.0,
    "s-4vcpu-8gb": 48.0,
}
DO_VOLUME_GB = 0.10
# Fly: shared CPU $1.94/mo each (256 MB included), RAM above that $5/GB/mo; volumes $0.15/GB/mo.
FLY_SIZES = {"shared-cpu-1x": (1, 1024), "shared-cpu-2x": (2, 2048), "shared-cpu-4x": (4, 4096)}
FLY_VOLUME_GB = 0.15
# AWS Lightsail Linux bundles (the plan in 224 §10) and block storage.
AWS_SIZES = {"nano_3_0": 5.0, "micro_3_0": 7.0, "small_3_0": 12.0, "medium_3_0": 24.0}
AWS_VOLUME_GB = 0.10

DEFAULTS = {
    "digitalocean": {"region": "lon1", "size": "s-1vcpu-1gb"},
    "fly": {"region": "lhr", "size": "shared-cpu-1x"},
    "aws": {"region": "eu-west-2", "size": "micro_3_0"},
    "kubernetes": {"region": "", "size": ""},
}


class CloudError(Exception):
    """A provider or input error; its message is what the model sees."""


def check_name(name):
    if not isinstance(name, str) or not NAME_RE.match(name):
        raise CloudError(f"name {name!r}: 1-40 of [a-z0-9-], starting with a letter, not ending in -")
    return name


def tag_node(name):
    return f"theuth-node:{name}"


# --- credentials ---------------------------------------------------------------------------------


def home():
    return Path(os.environ.get("HOME") or Path.home())


def _yaml_scalar(text, key):
    """`key: value` at any indent, first occurrence, quotes stripped (enough for doctl/fly files)."""
    m = re.search(rf"(?m)^\s*{re.escape(key)}:\s*['\"]?([^'\"\s#]+)", text)
    return m.group(1) if m else None


def do_token():
    """`(token, source)`; source names where it came from, never the value."""
    if os.environ.get("DIGITALOCEAN_TOKEN"):
        return os.environ["DIGITALOCEAN_TOKEN"], "env DIGITALOCEAN_TOKEN"
    for p in (home() / ".config/doctl/config.yaml",
              home() / "Library/Application Support/doctl/config.yaml"):
        if p.is_file():
            tok = _yaml_scalar(p.read_text(errors="replace"), "access-token")
            if tok:
                return tok, str(p)
    return None, None


def fly_token():
    for var in ("FLY_API_TOKEN", "FLY_ACCESS_TOKEN"):
        if os.environ.get(var):
            return os.environ[var], f"env {var}"
    p = home() / ".fly/config.yml"
    if p.is_file():
        tok = _yaml_scalar(p.read_text(errors="replace"), "access_token")
        if tok:
            return tok, str(p)
    return None, None


def aws_profiles():
    """Profile names from ~/.aws/credentials and ~/.aws/config (SSO included), plus the env."""
    names, sources = [], []
    if os.environ.get("AWS_ACCESS_KEY_ID"):
        names.append("(env)")
        sources.append("env AWS_ACCESS_KEY_ID")
    for f, pat in ((home() / ".aws/credentials", r"(?m)^\[([^\]]+)\]"),
                   (home() / ".aws/config", r"(?m)^\[(?:profile )?([^\]]+)\]")):
        if f.is_file():
            sources.append(str(f))
            for n in re.findall(pat, f.read_text(errors="replace")):
                if not n.startswith("sso-session") and n not in names:
                    names.append(n)
    return names, sources


def kube_contexts(run=subprocess.run):
    if not shutil.which("kubectl"):
        return [], None
    try:
        out = run(["kubectl", "config", "get-contexts", "-o", "name"], capture_output=True,
                  text=True, timeout=15)
    except (OSError, subprocess.SubprocessError):
        return [], None
    if out.returncode != 0:
        return [], None
    return [c for c in out.stdout.split() if c], os.environ.get("KUBECONFIG") or str(home() / ".kube/config")


def providers(run=subprocess.run):
    do_tok, do_src = do_token()
    fly_tok, fly_src = fly_token()
    aws_names, aws_src = aws_profiles()
    contexts, kubeconfig = kube_contexts(run)
    return {
        "digitalocean": {"configured": bool(do_tok), "credentials": do_src, "implemented": True,
                         "store_in_vault": "t --secret-set digitalocean.token"},
        "fly": {"configured": bool(fly_tok), "credentials": fly_src, "implemented": True,
                "org": os.environ.get("FLY_ORG", "personal"),
                "store_in_vault": "t --secret-set fly.token"},
        "kubernetes": {"configured": bool(contexts), "credentials": kubeconfig, "contexts": contexts,
                       "implemented": True, "via": "t mesh add --k8s"},
        "aws": {"configured": bool(aws_names), "credentials": ", ".join(aws_src) or None,
                "profiles": aws_names, "implemented": False,
                "plan": "Lightsail instance + Lightsail disk + closed firewall (design doc 224 §10)"},
    }


# --- HTTP ----------------------------------------------------------------------------------------


def http(method, url, token, body=None, timeout=60):
    """`(status, json-or-None)`. The token goes in a header and nowhere else."""
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("Authorization", f"Bearer {token}")
    req.add_header("Content-Type", "application/json")
    req.add_header("User-Agent", "theuth-cloud-nodes/0.1")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            raw = r.read()
            return r.status, (json.loads(raw) if raw.strip() else None)
    except urllib.error.HTTPError as e:
        raw = e.read()
        try:
            return e.code, json.loads(raw)
        except ValueError:
            return e.code, {"message": raw[:300].decode(errors="replace")}


class Api:
    """A JSON API with one bearer token; `transport` is replaceable for tests."""

    def __init__(self, base, token, transport=http):
        self.base, self._token, self.transport = base, token, transport

    def call(self, method, path, body=None, ok=(200, 201, 202, 204)):
        status, data = self.transport(method, self.base + path, self._token, body)
        if status not in ok:
            msg = (data or {}).get("message") or (data or {}).get("error") or data
            raise CloudError(f"{method} {path}: HTTP {status}: {msg}")
        return data or {}


# --- the pairing ticket --------------------------------------------------------------------------


def t_argv():
    """`t` with this engine's data dir (the plugin's data dir is `<data_dir>/plugin-data/<name>`)."""
    argv = [os.environ.get("THEUTH_BIN", "t")]
    pd = os.environ.get("THEUTH_PLUGIN_DATA")
    if pd:
        argv += ["--data-dir", str(Path(pd).parent.parent)]
    return argv


def strip_vault_key(pairing):
    """`t mesh pair --remote` for a `t` without it: the payload minus `vault_key`/`vault_key_id`."""
    s = pairing.strip()
    if s.startswith('"'):
        s = json.loads(s)
    version, _, body = s.partition(".")
    if version != "1" or not body:
        raise CloudError("the engine returned a malformed pairing string")
    b = body.upper()
    p = json.loads(base64.b32decode(b + "=" * (-len(b) % 8)))
    p.pop("vault_key", None)
    p.pop("vault_key_id", None)
    return "1." + base64.b32encode(json.dumps(p, separators=(",", ":")).encode()).decode().rstrip("=")


def mint_ticket(run=subprocess.run):
    """A one-time ticket (5 minutes) from this machine's engine, without its vault key."""
    argv = t_argv()
    try:
        has = run(argv + ["mesh", "pair", "--help"], capture_output=True, text=True, timeout=30)
        remote = "--remote" in (has.stdout or "")
        out = run(argv + ["mesh", "pair"] + (["--remote"] if remote else []), capture_output=True,
                  text=True, timeout=60)
    except (OSError, subprocess.SubprocessError) as e:
        raise CloudError(f"`t mesh pair`: {e}") from None
    if out.returncode != 0:
        raise CloudError(f"`t mesh pair` failed: {out.stderr.strip()[-300:]}")
    ticket = out.stdout.strip() if remote else strip_vault_key(out.stdout)
    if not TICKET_RE.match(ticket):
        raise CloudError("`t mesh pair` printed something that is not a pairing string")
    return ticket


def mesh_peer(name, run=subprocess.run):
    """This mesh's peer called `name`, or None (no engine → None)."""
    try:
        out = run(t_argv() + ["mesh", "status"], capture_output=True, text=True, timeout=30)
        peers = json.loads(out.stdout).get("peers", []) if out.returncode == 0 else []
    except (OSError, subprocess.SubprocessError, ValueError):
        return None
    return next(({"id": p.get("id"), "reachable": p.get("reachable")} for p in peers
                 if p.get("name") == name), None)


# --- native VM install (DigitalOcean; Hetzner and Lightsail would reuse it) ----------------------

# Copies of deploy/computer/vm-cloud-init.sh, deploy/computer/theuth-install.sh and
# crates/update/pubkey.txt (an installed plugin has no repo beside it; the tests check the copies).
VM_DIR = Path(__file__).resolve().parent / "vm"


def user_data(name, data_dev, ticket, manifest=MANIFEST):
    """The VM's first-boot script (design doc 224 §9): user `engine`, the volume at /data, the
    runtime libraries, the newest signed `t` into /data/bin (theuth-install.sh: minisign against
    the release key), the ticket at /data/pair-ticket, systemd units theuth-engine (Restart=always)
    and theuth-join. No Docker."""
    if ticket and not TICKET_RE.match(ticket):
        raise CloudError("not a pairing ticket")
    if not re.match(r"^[A-Za-z0-9/_.:-]+$", data_dev) or not re.match(r"^https://[A-Za-z0-9/_.:-]+$", manifest):
        raise CloudError("bad device path or manifest URL")
    subs = {
        "__NAME__": check_name(name),
        "__DATA_DEV__": data_dev,
        "__TICKET__": ticket or "",
        "__MANIFEST__": manifest,
        "__PUBKEY__": (VM_DIR / "pubkey.txt").read_text().strip(),
        "__INSTALL_SH__": base64.b64encode((VM_DIR / "theuth-install.sh").read_bytes()).decode(),
    }
    text = (VM_DIR / "vm-cloud-init.sh").read_text()
    for k, v in subs.items():
        text = text.replace(k, v)
    return text


# --- DigitalOcean --------------------------------------------------------------------------------


class DigitalOcean:
    def __init__(self, transport=http, token=None):
        tok = token or do_token()[0]
        if not tok:
            raise CloudError("no DigitalOcean token: `t --secret-set digitalocean.token` and add "
                             "DIGITALOCEAN_TOKEN: \"{{secret:digitalocean.token}}\" to plugin.yaml env, "
                             "or `doctl auth init`")
        self.api = Api(DO_API, tok, transport)

    @staticmethod
    def estimate(size, volume_gb):
        if size not in DO_SIZES:
            raise CloudError(f"size {size!r}: one of {', '.join(DO_SIZES)}")
        return round(DO_SIZES[size] + DO_VOLUME_GB * volume_gb, 2)

    def droplet(self, name):
        tag = urllib.request.quote(tag_node(name), safe="")
        ds = self.api.call("GET", f"/droplets?tag_name={tag}").get("droplets", [])
        return ds[0] if ds else None

    def volume(self, name, region=None):
        q = f"/volumes?name=theuth-{name}-data" + (f"&region={region}" if region else "")
        vs = self.api.call("GET", q).get("volumes", [])
        return vs[0] if vs else None

    def firewall(self, name):
        fws = self.api.call("GET", "/firewalls?per_page=200").get("firewalls", [])
        return next((f for f in fws if f["name"] == f"theuth-{name}"), None)

    def create(self, name, region, size, volume_gb, ticket, ssh_cidr=None, ssh_key=None, manifest=MANIFEST):
        if self.droplet(name):
            raise CloudError(f"a Droplet tagged {tag_node(name)} exists; destroy it first")
        for t in (TAG_MANAGED, tag_node(name)):
            self.api.call("POST", "/tags", {"name": t}, ok=(201, 422))
        if not self.firewall(name):
            out = {"addresses": ["0.0.0.0/0", "::/0"]}
            self.api.call("POST", "/firewalls", {
                "name": f"theuth-{name}",
                "tags": [tag_node(name)],
                "inbound_rules": ([{"protocol": "tcp", "ports": "22", "sources": {"addresses": [ssh_cidr]}}]
                                  if ssh_cidr else []),
                "outbound_rules": [
                    {"protocol": "tcp", "ports": "all", "destinations": out},
                    {"protocol": "udp", "ports": "all", "destinations": out},
                    {"protocol": "icmp", "destinations": out},
                ],
            })
        vol = self.volume(name, region)
        if not vol:
            vol = self.api.call("POST", "/volumes", {
                "name": f"theuth-{name}-data", "region": region, "size_gigabytes": volume_gb,
                "filesystem_type": "ext4", "filesystem_label": "theuth-data",
                "tags": [TAG_MANAGED, tag_node(name)],
                "description": "theuth node /data (design doc 224)",
            })["volume"]
        body = {
            "name": f"theuth-{name}", "region": region, "size": size,
            # vm-cloud-init.sh names Ubuntu 24.04's packages.
            "image": "ubuntu-24-04-x64",
            "volumes": [vol["id"]], "tags": [TAG_MANAGED, tag_node(name)], "ipv6": True,
            "monitoring": False,
            "user_data": user_data(name, f"/dev/disk/by-id/scsi-0DO_Volume_theuth-{name}-data", ticket, manifest),
        }
        if ssh_key:
            body["ssh_keys"] = [int(ssh_key) if str(ssh_key).isdigit() else ssh_key]
        d = self.api.call("POST", "/droplets", body)["droplet"]
        return {"droplet_id": d["id"], "volume_id": vol["id"], "firewall": f"theuth-{name}"}

    def wait_active(self, droplet_id, tries=36, pause=5.0, sleep=time.sleep):
        for _ in range(tries):
            d = self.api.call("GET", f"/droplets/{droplet_id}")["droplet"]
            if d["status"] == "active":
                return _public_ip(d)
            sleep(pause)
        return None

    def status(self, name):
        d = self.droplet(name)
        v = self.volume(name)
        f = self.firewall(name)
        return {
            "droplet": d and {"id": d["id"], "status": d["status"], "size": d["size_slug"],
                              "region": d["region"]["slug"], "ip": _public_ip(d), "created": d["created_at"]},
            "volume": v and {"id": v["id"], "size_gb": v["size_gigabytes"], "attached": v["droplet_ids"]},
            "firewall": f and {"id": f["id"], "status": f["status"],
                               "inbound": [r.get("ports") for r in f.get("inbound_rules", [])]},
        }

    def list(self):
        ds = self.api.call("GET", f"/droplets?tag_name={TAG_MANAGED}&per_page=200").get("droplets", [])
        vs = self.api.call("GET", "/volumes?per_page=200").get("volumes", [])
        nodes = {}
        for d in ds:
            n = _node_of(d.get("tags", []))
            if n:
                nodes[n] = {"provider": "digitalocean", "name": n, "status": d["status"],
                            "region": d["region"]["slug"], "ip": _public_ip(d)}
        for v in vs:
            n = _node_of(v.get("tags") or [])
            if n and TAG_MANAGED in (v.get("tags") or []) and n not in nodes:
                nodes[n] = {"provider": "digitalocean", "name": n, "status": "volume kept",
                            "region": v["region"]["slug"]}
        return list(nodes.values())

    def destroy(self, name, keep_volume, tries=60, sleep=time.sleep):
        done = []
        d = self.droplet(name)
        if d:
            self.api.call("DELETE", f"/droplets/{d['id']}")
            done.append(f"droplet {d['id']} deleted")
        f = self.firewall(name)
        if f:
            self.api.call("DELETE", f"/firewalls/{f['id']}")
            done.append(f"firewall {f['id']} deleted")
        v = self.volume(name)
        if v and not keep_volume:
            for _ in range(tries):
                if not self.api.call("GET", f"/volumes/{v['id']}")["volume"]["droplet_ids"]:
                    break
                sleep(5)
            self.api.call("DELETE", f"/volumes/{v['id']}")
            done.append(f"volume {v['name']} deleted")
        elif v:
            done.append(f"volume {v['name']} kept (${DO_VOLUME_GB * v['size_gigabytes']:.2f}/month)")
        self.api.call("DELETE", "/tags/" + urllib.request.quote(tag_node(name), safe=""), ok=(204, 404, 422))
        return done


def _public_ip(d):
    return next((n["ip_address"] for n in d.get("networks", {}).get("v4", []) if n["type"] == "public"), None)


def _node_of(tags):
    return next((t.split(":", 1)[1] for t in tags if t.startswith("theuth-node:")), None)


# --- Fly.io --------------------------------------------------------------------------------------


class Fly:
    """One Fly app per node, `theuth-<name>-<6 hex of the org>` (app names are global); a volume
    at /data (Fly allows one per Machine, so /workspace is the image's, i.e. scratch); restart
    always. The image starts as root only to own the root-formatted volume, then drops to uid 1000
    (deploy/computer/entrypoint.sh step 1)."""

    def __init__(self, transport=http, token=None, org=None):
        tok = token or fly_token()[0]
        if not tok:
            raise CloudError("no Fly.io token: `t --secret-set fly.token` and add "
                             "FLY_API_TOKEN: \"{{secret:fly.token}}\" to plugin.yaml env, or `fly auth login`")
        self.api = Api(FLY_API, tok, transport)
        self.org = org or os.environ.get("FLY_ORG", "personal")

    def app(self, name):
        return f"theuth-{name}-{hashlib.sha256(self.org.encode()).hexdigest()[:6]}"

    @staticmethod
    def guest(size):
        if size not in FLY_SIZES:
            raise CloudError(f"size {size!r}: one of {', '.join(FLY_SIZES)}")
        cpus, mem = FLY_SIZES[size]
        return {"cpu_kind": "shared", "cpus": cpus, "memory_mb": mem}

    @staticmethod
    def estimate(size, volume_gb):
        g = Fly.guest(size)
        extra_gb = max(0, g["memory_mb"] - 256 * g["cpus"]) / 1024
        return round(1.94 * g["cpus"] + 5.0 * extra_gb + FLY_VOLUME_GB * volume_gb, 2)

    def create(self, name, region, size, volume_gb, image, ticket):
        app = self.app(name)
        self.api.call("POST", "/apps", {"app_name": app, "org_slug": self.org})
        vol = self.api.call("POST", f"/apps/{app}/volumes", {
            "name": "theuth_data", "region": region, "size_gb": volume_gb, "encrypted": True,
        })
        config = {
            "image": image,
            "guest": self.guest(size),
            "env": {"THEUTH_NODE_NAME": name},
            "mounts": [{"volume": vol["id"], "path": "/data"}],
            "restart": {"policy": "always"},
            "processes": [{"name": "engine", "user": "root"}],
            "metadata": {TAG_MANAGED: "true", "theuth-node": name},
        }
        if ticket:
            config["files"] = [{"guest_path": "/run/theuth/pair/ticket",
                                "raw_value": base64.b64encode(ticket.encode()).decode(), "mode": 0o444}]
        m = self.api.call("POST", f"/apps/{app}/machines", {"name": name, "region": region, "config": config})
        return {"app": app, "machine_id": m.get("id"), "volume_id": vol["id"]}

    def status(self, name):
        app = self.app(name)
        ms = self.api.call("GET", f"/apps/{app}/machines", ok=(200, 404))
        vs = self.api.call("GET", f"/apps/{app}/volumes", ok=(200, 404))
        ms = ms if isinstance(ms, list) else []
        vs = vs if isinstance(vs, list) else []
        return {"app": app,
                "machines": [{"id": m["id"], "state": m.get("state"), "region": m.get("region"),
                              "image": (m.get("config") or {}).get("image")} for m in ms],
                "volumes": [{"id": v["id"], "size_gb": v.get("size_gb"), "state": v.get("state")} for v in vs]}

    def list(self):
        apps = self.api.call("GET", f"/apps?org_slug={self.org}").get("apps", [])
        suffix = "-" + hashlib.sha256(self.org.encode()).hexdigest()[:6]
        return [{"provider": "fly", "name": a["name"][len("theuth-"):-len(suffix)], "app": a["name"],
                 "machines": a.get("machine_count")}
                for a in apps if a["name"].startswith("theuth-") and a["name"].endswith(suffix)]

    def destroy(self, name, keep_volume):
        app = self.app(name)
        done = []
        ms = self.api.call("GET", f"/apps/{app}/machines", ok=(200, 404))
        for m in ms if isinstance(ms, list) else []:
            self.api.call("DELETE", f"/apps/{app}/machines/{m['id']}?force=true")
            done.append(f"machine {m['id']} deleted")
        if keep_volume:
            done.append(f"app {app} and its volume kept (${FLY_VOLUME_GB}/GB/month)")
            return done
        vs = self.api.call("GET", f"/apps/{app}/volumes", ok=(200, 404))
        for v in vs if isinstance(vs, list) else []:
            self.api.call("DELETE", f"/apps/{app}/volumes/{v['id']}")
            done.append(f"volume {v['id']} deleted")
        self.api.call("DELETE", f"/apps/{app}", ok=(200, 202, 204, 404))
        done.append(f"app {app} deleted")
        return done


# --- Kubernetes (via `t`) ------------------------------------------------------------------------


def k8s_args(name, context=None, namespace=None, storage_class=None, volume_gb=None, image=None,
             deny_cluster_network=False, dry_run=False):
    argv = ["mesh", "add", "--k8s", "--name", name]
    for flag, v in (("--context", context), ("--namespace", namespace), ("--storage-class", storage_class),
                    ("--image", image)):
        if v:
            argv += [flag, v]
    if volume_gb:
        argv += ["--volume-size", f"{int(volume_gb)}Gi"]
    if deny_cluster_network:
        argv.append("--deny-cluster-network")
    if dry_run:
        argv.append("--dry-run")
    return argv


def run_t(args, run=subprocess.run, timeout=900):
    try:
        out = run(t_argv() + args, capture_output=True, text=True, timeout=timeout)
    except (OSError, subprocess.SubprocessError) as e:
        raise CloudError(f"`t {' '.join(args)}`: {e}") from None
    if out.returncode != 0:
        raise CloudError(f"`t {' '.join(args)}` failed: {(out.stderr or out.stdout).strip()[-600:]}")
    return out.stdout


# --- the tools -----------------------------------------------------------------------------------


def create(args, transport=http, run=subprocess.run, sleep=time.sleep):
    provider = args.get("provider")
    if provider not in DEFAULTS:
        raise CloudError(f"provider {provider!r}: one of {', '.join(DEFAULTS)}")
    name = check_name(args.get("name"))
    region = args.get("region") or DEFAULTS[provider]["region"]
    size = args.get("size") or DEFAULTS[provider]["size"]
    volume_gb = int(args.get("volume_gb") or 10)
    image = args.get("image") or ENGINE_IMAGE
    dry_run = args.get("dry_run", True)

    if provider == "kubernetes":
        argv = k8s_args(name, args.get("context"), args.get("namespace"), args.get("storage_class"),
                        volume_gb, args.get("image"), args.get("deny_cluster_network", False), dry_run)
        out = run_t(argv, run)
        return {"provider": provider, "name": name, "dry_run": dry_run,
                "estimated_monthly_usd": None, "cost_note": "your cluster's",
                "output": out if dry_run else out.strip().splitlines()[-1:]}

    if provider == "aws":
        if size not in AWS_SIZES:
            raise CloudError(f"size {size!r}: one of {', '.join(AWS_SIZES)}")
        plan = {"provider": "aws", "name": name, "region": region, "size": size, "volume_gb": volume_gb,
                "estimated_monthly_usd": round(AWS_SIZES[size] + AWS_VOLUME_GB * volume_gb, 2),
                "implemented": False,
                "plan": ["lightsail CreateDisk theuth-<name>-data (tags theuth-managed, theuth-node:<name>)",
                         ("lightsail CreateInstances ubuntu_24_04, bundle <size>, user-data = vm/vm-cloud-init.sh "
                          "(native install, as DigitalOcean) with the disk at /dev/nvme1n1"),
                         ("lightsail AttachDisk; PutInstancePublicPorts [] (no inbound; 22 from one CIDR "
                          "if asked)")]}
        if not dry_run:
            raise CloudError("AWS create is not implemented yet (design doc 224 §10); dry run: " + json.dumps(plan))
        return plan

    # VM providers install natively; container providers run the engine image (224 §2).
    if provider == "digitalocean" and args.get("image"):
        raise CloudError("DigitalOcean installs t natively (no Docker); `image` is for fly and kubernetes")
    est = DigitalOcean.estimate(size, volume_gb) if provider == "digitalocean" else Fly.estimate(size, volume_gb)
    summary = {"provider": provider, "name": name, "region": region, "size": size, "volume_gb": volume_gb,
               "install": "native: systemd unit theuth-engine, user engine" if provider == "digitalocean" else image,
               "estimated_monthly_usd": est, "dry_run": dry_run,
               "inbound": f"tcp/22 from {args['ssh_cidr']}" if args.get("ssh_cidr") else "none"}
    if dry_run:
        summary["next"] = "call create again with dry_run: false to make it (billing starts then)"
        return summary

    ticket = mint_ticket(run)
    if provider == "digitalocean":
        do = DigitalOcean(transport)
        made = do.create(name, region, size, volume_gb, ticket, args.get("ssh_cidr"), args.get("ssh_key"))
        made["ip"] = do.wait_active(made["droplet_id"], sleep=sleep)
    else:
        made = Fly(transport).create(name, region, size, volume_gb, image, ticket)
    summary.update(made)
    summary["next"] = (f"the node joins within ~5 min (the ticket's lifetime); status(name: {name}) "
                       "shows the instance and whether it is a mesh peer")
    return summary


def _provider_obj(provider, transport):
    if provider == "digitalocean":
        return DigitalOcean(transport)
    if provider == "fly":
        return Fly(transport)
    raise CloudError(f"provider {provider!r}: digitalocean, fly or kubernetes")


def status(args, transport=http, run=subprocess.run):
    name = check_name(args.get("name"))
    provider = args.get("provider") or "digitalocean"
    if provider == "kubernetes":
        argv = ["mesh", "k8s", "status", name]
        for flag in ("context", "namespace"):
            if args.get(flag):
                argv += [f"--{flag}", args[flag]]
        out = {"provider": provider, "objects": run_t(argv, run)}
    else:
        out = {"provider": provider, **_provider_obj(provider, transport).status(name)}
    out["mesh_peer"] = mesh_peer(name, run)
    return out


def list_nodes(args, transport=http, run=subprocess.run):
    """Every node of every configured provider; a provider that fails is reported, not fatal."""
    nodes, errors = [], {}
    p = providers(run)
    for prov, cls in (("digitalocean", DigitalOcean), ("fly", Fly)):
        if p[prov]["configured"]:
            try:
                nodes += cls(transport).list()
            except CloudError as e:
                errors[prov] = str(e)
    if p["kubernetes"]["configured"]:
        try:
            for line in run_t(["mesh", "k8s", "list"], run).splitlines():
                if "/" in line:
                    ns_name, _, rest = line.partition("  ")
                    nodes.append({"provider": "kubernetes", "name": ns_name.split("/", 1)[1],
                                  "namespace": ns_name.split("/", 1)[0], "status": rest.strip()})
        except CloudError as e:
            errors["kubernetes"] = str(e)
    return {"nodes": nodes, "errors": errors}


def destroy(args, transport=http, run=subprocess.run):
    name = check_name(args.get("name"))
    provider = args.get("provider") or "digitalocean"
    keep = bool(args.get("keep_volume", True))
    if provider == "kubernetes":
        argv = ["mesh", "k8s", "destroy", name] + ([] if keep else ["--delete-volume"])
        for flag in ("context", "namespace"):
            if args.get(flag):
                argv += [f"--{flag}", args[flag]]
        done = run_t(argv, run).strip().splitlines()
    else:
        done = _provider_obj(provider, transport).destroy(name, keep)
    peer = mesh_peer(name, run)
    return {"provider": provider, "name": name, "done": done,
            "mesh": (f"still a peer: `t mesh remove {peer['id']}` takes it out" if peer else "not a peer")}
