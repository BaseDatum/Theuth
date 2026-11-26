"""Tests for cloud-nodes: no network, no cloud, no engine. `python3 -m unittest -v` in this folder."""

import base64
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import cloud_nodes as cn

HERE = Path(__file__).resolve().parent
TICKET = "1." + base64.b32encode(json.dumps({"id": "abc", "token": "T0KEN"}).encode()).decode().rstrip("=")


class Fake:
    """A scripted transport: `routes` maps (METHOD, path prefix) to (status, body); calls are kept."""

    def __init__(self, routes):
        self.routes, self.calls = routes, []

    def __call__(self, method, url, token, body=None):
        self.calls.append((method, url, body))
        path = url.split("/v2", 1)[-1] if "/v2" in url else url.split("/v1", 1)[-1]
        for (m, prefix), reply in self.routes.items():
            if m == method and path.startswith(prefix):
                return reply(body) if callable(reply) else reply
        return 404, {"message": f"no route {method} {path}"}

    def bodies(self, method, prefix):
        return [b for m, u, b in self.calls if m == method and prefix in u]


def fake_t(pairing, peers=()):
    """`subprocess.run` for `t`: an old `t` (no --remote) that prints `pairing` as a JSON string."""

    def run(argv, **_):
        cmd = argv[argv.index("mesh") + 1:] if "mesh" in argv else argv[1:]
        if cmd[:2] == ["pair", "--help"]:
            return subprocess.CompletedProcess(argv, 0, "Print a pairing string\n", "")
        if cmd[:1] == ["pair"]:
            return subprocess.CompletedProcess(argv, 0, json.dumps(pairing) + "\n", "")
        if cmd[:1] == ["status"]:
            return subprocess.CompletedProcess(argv, 0, json.dumps({"peers": list(peers)}), "")
        if argv[0] == "kubectl":
            return subprocess.CompletedProcess(argv, 0, "k8s.example\n", "")
        return subprocess.CompletedProcess(argv, 0, "ok\n", "")

    return run


def pairing_with_key():
    p = {"id": "abc", "name": "mac", "relay": "https://iroh.relay.theuth.io", "addrs": [],
         "token": "T0KEN", "vault_key": "SECRETKEY", "vault_key_id": "k1"}
    return "1." + base64.b32encode(json.dumps(p).encode()).decode().rstrip("=")


class Env(unittest.TestCase):
    def setUp(self):
        self.home = tempfile.TemporaryDirectory()
        patch = mock.patch.dict(os.environ, {"HOME": self.home.name}, clear=False)
        patch.start()
        self.addCleanup(patch.stop)
        self.addCleanup(self.home.cleanup)
        for var in ("DIGITALOCEAN_TOKEN", "FLY_API_TOKEN", "FLY_ACCESS_TOKEN", "AWS_ACCESS_KEY_ID",
                    "KUBECONFIG", "THEUTH_PLUGIN_DATA", "CLOUD_NODES_ENGINE_CONTEXT"):
            os.environ.pop(var, None)


class Credentials(Env):
    def test_discovers_cli_files_and_never_returns_values(self):
        h = Path(self.home.name)
        (h / ".config/doctl").mkdir(parents=True)
        (h / ".config/doctl/config.yaml").write_text("access-token: dop_v1_SECRET\ncontext: default\n")
        (h / ".fly").mkdir()
        (h / ".fly/config.yml").write_text("access_token: fo1_SECRET\n")
        (h / ".aws").mkdir()
        (h / ".aws/config").write_text("[profile dev]\nsso_session = x\n[sso-session x]\n")
        (h / ".aws/credentials").write_text("[default]\naws_access_key_id = AKIASECRET\n")
        with mock.patch("shutil.which", return_value=None):
            p = cn.providers()
        self.assertTrue(p["digitalocean"]["configured"])
        self.assertTrue(p["fly"]["configured"])
        self.assertEqual(p["aws"]["profiles"], ["default", "dev"])
        self.assertFalse(p["kubernetes"]["configured"])
        self.assertNotIn("SECRET", json.dumps(p))

    def test_environment_wins_over_files(self):
        os.environ["DIGITALOCEAN_TOKEN"] = "from-vault"
        self.assertEqual(cn.do_token(), ("from-vault", "env DIGITALOCEAN_TOKEN"))


class Ticket(Env):
    def test_strips_the_vault_key_and_keeps_the_token(self):
        t = cn.mint_ticket(fake_t(pairing_with_key()))
        b = t.split(".", 1)[1]
        p = json.loads(base64.b32decode(b + "=" * (-len(b) % 8)))
        self.assertEqual(p["token"], "T0KEN")
        self.assertNotIn("vault_key", p)
        self.assertNotIn("vault_key_id", p)

    def test_uses_the_engine_data_dir(self):
        os.environ["THEUTH_PLUGIN_DATA"] = "/srv/theuth/plugin-data/cloud-nodes"
        self.assertEqual(cn.t_argv()[1:], ["--data-dir", "/srv/theuth"])


class DigitalOceanCreate(Env):
    def setUp(self):
        super().setUp()
        os.environ["DIGITALOCEAN_TOKEN"] = "tok"

    def routes(self):
        return {
            ("GET", "/droplets/"): (200, {"droplet": {"id": 7, "status": "active", "networks": {
                "v4": [{"type": "public", "ip_address": "203.0.113.9"}]}}}),
            ("GET", "/droplets"): (200, {"droplets": []}),
            ("POST", "/tags"): (201, {}),
            ("GET", "/firewalls"): (200, {"firewalls": []}),
            ("POST", "/firewalls"): (202, {"firewall": {"id": "fw"}}),
            ("GET", "/volumes"): (200, {"volumes": []}),
            ("POST", "/volumes"): (201, {"volume": {"id": "vol-1"}}),
            ("POST", "/droplets"): (202, {"droplet": {"id": 7}}),
        }

    def test_dry_run_estimates_and_creates_nothing(self):
        fake = Fake({})
        out = cn.create({"provider": "digitalocean", "name": "lab"}, transport=fake)
        self.assertEqual(out["estimated_monthly_usd"], 7.0)
        self.assertTrue(out["dry_run"])
        self.assertEqual(fake.calls, [])

    def test_firewall_before_droplet_no_inbound_ticket_only_in_user_data(self):
        fake = Fake(self.routes())
        out = cn.create({"provider": "digitalocean", "name": "lab", "dry_run": False},
                        transport=fake, run=fake_t(pairing_with_key()), sleep=lambda _: None)
        self.assertEqual(out["ip"], "203.0.113.9")
        order = [(m, u.split("/v2")[1].split("?")[0]) for m, u, _ in fake.calls if m == "POST"]
        self.assertEqual(order, [("POST", "/tags"), ("POST", "/tags"), ("POST", "/firewalls"),
                                 ("POST", "/volumes"), ("POST", "/droplets")])
        fw = fake.bodies("POST", "/firewalls")[0]
        self.assertEqual(fw["inbound_rules"], [])
        self.assertEqual(fw["tags"], ["theuth-node:lab"])
        d = fake.bodies("POST", "/droplets")[0]
        self.assertEqual(d["volumes"], ["vol-1"])
        self.assertEqual(d["tags"], ["theuth-managed", "theuth-node:lab"])
        self.assertIn("docker run -d --name theuth-engine --restart always --init", d["user_data"])
        self.assertIn("/mnt/theuth/data/pair-ticket", d["user_data"])
        self.assertNotIn("SECRETKEY", json.dumps(fake.calls))
        self.assertNotIn("T0KEN", json.dumps(out))

    def test_ssh_is_one_cidr_and_existing_node_is_refused(self):
        fake = Fake(self.routes())
        cn.create({"provider": "digitalocean", "name": "lab", "dry_run": False, "ssh_cidr": "198.51.100.4/32"},
                  transport=fake, run=fake_t(pairing_with_key()), sleep=lambda _: None)
        rule = fake.bodies("POST", "/firewalls")[0]["inbound_rules"]
        self.assertEqual(rule, [{"protocol": "tcp", "ports": "22", "sources": {"addresses": ["198.51.100.4/32"]}}])
        busy = dict(self.routes())
        busy[("GET", "/droplets")] = (200, {"droplets": [{"id": 1}]})
        with self.assertRaises(cn.CloudError):
            cn.create({"provider": "digitalocean", "name": "lab", "dry_run": False},
                      transport=Fake(busy), run=fake_t(pairing_with_key()))

    def test_user_data_is_valid_bash_in_both_image_modes(self):
        for image in ("ghcr.io/basedatum/theuth/engine:2.31.2", "build"):
            ud = cn.user_data("lab", "theuth-lab-data", image, TICKET, "2.31.2", "PUBKEY",
                              context=HERE.parent.parent / "deploy/computer")
            r = subprocess.run(["bash", "-n"], check=False, input=ud, text=True, capture_output=True)
            self.assertEqual(r.returncode, 0, r.stderr)
            self.assertNotRegex(ud, r"(?m)^\s*set -[a-z]*x")
        with self.assertRaises(cn.CloudError):
            cn.user_data("lab", "v", "img; rm -rf /", None)
        with self.assertRaises(cn.CloudError):
            cn.user_data("lab", "v", "img", "'; reboot; '")


class DigitalOceanDestroy(Env):
    def test_keep_volume_keeps_it_and_default_waits_for_detach(self):
        os.environ["DIGITALOCEAN_TOKEN"] = "tok"
        detached = iter([["7"], []])
        routes = {
            ("GET", "/droplets"): (200, {"droplets": [{"id": 7}]}),
            ("DELETE", "/droplets/"): (204, None),
            ("GET", "/firewalls"): (200, {"firewalls": [{"id": "fw", "name": "theuth-lab"}]}),
            ("DELETE", "/firewalls/"): (204, None),
            ("GET", "/volumes/"): lambda _: (200, {"volume": {"droplet_ids": next(detached)}}),
            ("GET", "/volumes"): (200, {"volumes": [{"id": "v1", "name": "theuth-lab-data", "size_gigabytes": 10}]}),
            ("DELETE", "/volumes/"): (204, None),
            ("DELETE", "/tags/"): (204, None),
        }
        fake = Fake(routes)
        out = cn.DigitalOcean(fake).destroy("lab", keep_volume=True)
        self.assertIn("volume theuth-lab-data kept ($1.00/month)", out)
        self.assertFalse(any(m == "DELETE" and "/volumes/" in u for m, u, _ in fake.calls))
        fake = Fake(routes)
        out = cn.DigitalOcean(fake).destroy("lab", keep_volume=False, sleep=lambda _: None)
        self.assertIn("volume theuth-lab-data deleted", out)


class FlyCreate(Env):
    def test_app_volume_machine_with_restart_always_and_the_ticket_as_a_file(self):
        os.environ["FLY_API_TOKEN"] = "fo1"
        fake = Fake({("POST", "/apps/"): lambda b: (200, {"id": "vol_1"} if "size_gb" in b else {"id": "m1"}),
                     ("POST", "/apps"): (201, {})})
        out = cn.Fly(fake, org="personal").create("lab", "lhr", "shared-cpu-1x", 10,
                                                   "ghcr.io/basedatum/theuth/engine:2.31.2", TICKET)
        self.assertTrue(out["app"].startswith("theuth-lab-"))
        cfg = fake.bodies("POST", "/machines")[0]["config"]
        self.assertEqual(cfg["restart"], {"policy": "always"})
        self.assertEqual(cfg["mounts"], [{"volume": "vol_1", "path": "/data"}])
        self.assertEqual(base64.b64decode(cfg["files"][0]["raw_value"]).decode(), TICKET)
        self.assertEqual(cn.Fly.estimate("shared-cpu-1x", 10), 7.19)


class Kubernetes(Env):
    def test_create_and_destroy_go_through_t(self):
        seen = []

        def run(argv, **kw):
            seen.append(argv)
            return fake_t(pairing_with_key())(argv, **kw)

        cn.create({"provider": "kubernetes", "name": "lab", "context": "c", "namespace": "n",
                   "storage_class": "nfs-csi", "deny_cluster_network": True}, run=run)
        self.assertEqual(seen[0][1:], ["mesh", "add", "--k8s", "--name", "lab", "--context", "c",
                                       "--namespace", "n", "--storage-class", "nfs-csi",
                                       "--volume-size", "10Gi", "--deny-cluster-network", "--dry-run"])
        cn.destroy({"provider": "kubernetes", "name": "lab", "keep_volume": False}, run=run)
        self.assertIn(["t", "mesh", "k8s", "destroy", "lab", "--delete-volume"], seen)


class Aws(Env):
    def test_dry_run_only(self):
        plan = cn.create({"provider": "aws", "name": "lab"})
        self.assertEqual(plan["estimated_monthly_usd"], 8.0)
        self.assertFalse(plan["implemented"])
        with self.assertRaises(cn.CloudError):
            cn.create({"provider": "aws", "name": "lab", "dry_run": False})


class Names(unittest.TestCase):
    def test_names(self):
        for ok in ("lab", "a1-b2"):
            cn.check_name(ok)
        for bad in ("", "Lab", "1a", "a-", "a_b", "a" * 41, None):
            with self.assertRaises(cn.CloudError):
                cn.check_name(bad)


class Wiring(Env):
    """The SDK path: tools/call → handler → CallToolResult; errors reach the model as isError."""

    def test_tools_answer_through_the_sdk(self):
        import main
        hello = {"jsonrpc": "2.0", "id": 0, "method": "hello",
                 "params": {"protocol": 1, "grants": {}, "data_dir": self.home.name}}
        call = {"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": {"name": "create", "arguments": {"provider": "digitalocean", "name": "lab"}}}
        bad = {"jsonrpc": "2.0", "id": 2, "method": "tools/call",
               "params": {"name": "create", "arguments": {"provider": "nope", "name": "lab"}}}
        out = main.plugin.drive([hello, call, bad])
        replies = {m.get("id"): m for m in out if "id" in m}
        self.assertEqual(replies[1]["result"]["structuredContent"]["estimated_monthly_usd"], 7.0)
        self.assertTrue(replies[2]["result"]["isError"])

    def test_manifest_and_code_declare_the_same_tools(self):
        r = subprocess.run(["python3", "main.py", "--theuth-manifest"], check=False, cwd=HERE, text=True, capture_output=True)
        self.assertEqual(r.returncode, 0, r.stderr)
        code = json.loads(next(line for line in r.stdout.splitlines() if line.startswith("tools:")).split(":", 1)[1])
        yaml_tools = [line.split("name:", 1)[1].strip() for line in (HERE / "plugin.yaml").read_text().splitlines()
                      if line.startswith("  - name:")]
        self.assertEqual(sorted(code), sorted(yaml_tools))


if __name__ == "__main__":
    unittest.main()
