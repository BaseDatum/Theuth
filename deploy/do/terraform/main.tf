locals {
  tags        = [digitalocean_tag.theuth.name, digitalocean_tag.infra.name]
  volume_name = "${var.name}-data"
  anywhere    = ["0.0.0.0/0", "::/0"]
}

resource "digitalocean_tag" "theuth" {
  name = "theuth"
}

resource "digitalocean_tag" "infra" {
  name = "theuth-infra"
}

resource "digitalocean_ssh_key" "admin" {
  name       = "${var.name}-admin"
  public_key = trimspace(file(pathexpand(var.ssh_public_key_path)))
}

# All persistent state lives here (/srv/theuth): the share edge's iroh.key, ACME account and names DB, and Caddy's
# certificates. Losing it means a new edge key (shipped clients pin the old one) and a re-registered ACME account
# (the tun.zip CAA records pin the old account URI): never destroy it with Terraform.
resource "digitalocean_volume" "data" {
  name                     = local.volume_name
  region                   = var.region
  size                     = var.volume_size_gib
  initial_filesystem_type  = "ext4"
  initial_filesystem_label = "theuth-data"
  description              = "theuth persistent data, mounted at /srv/theuth"
  tags                     = local.tags

  lifecycle {
    prevent_destroy = true
  }
}

resource "digitalocean_droplet" "host" {
  name       = var.name
  image      = var.image
  region     = var.region
  size       = var.size
  monitoring = true
  ipv6       = false
  ssh_keys   = [digitalocean_ssh_key.admin.fingerprint]
  tags       = local.tags
  # Attached at create so the volume is present on first boot (cloud-init mounts it).
  volume_ids = [digitalocean_volume.data.id]

  # No secrets here: user_data is stored in DO metadata and in the Terraform state.
  user_data = templatefile("${path.module}/cloud-init.yaml.tftpl", {
    volume_name = local.volume_name
  })

  lifecycle {
    # A changed cloud-init or a newer image would replace the droplet; host changes go through deploy.sh instead.
    ignore_changes = [user_data, image]
  }
}

resource "digitalocean_reserved_ip" "public" {
  region = var.region
}

resource "digitalocean_reserved_ip_assignment" "public" {
  ip_address = digitalocean_reserved_ip.public.ip_address
  droplet_id = digitalocean_droplet.host.id
}

resource "digitalocean_firewall" "host" {
  name        = var.name
  droplet_ids = [digitalocean_droplet.host.id]

  inbound_rule {
    protocol         = "tcp"
    port_range       = "22"
    source_addresses = var.admin_cidrs
  }

  # 80: Caddy's redirect to https. 443: HAProxy SNI router (theuth.io names, iroh-relay, *.tun.zip edge).
  inbound_rule {
    protocol         = "tcp"
    port_range       = "80"
    source_addresses = local.anywhere
  }

  inbound_rule {
    protocol         = "tcp"
    port_range       = "443"
    source_addresses = local.anywhere
  }

  # iroh-relay QUIC address discovery (iroh-relay.toml quic_bind_addr).
  inbound_rule {
    protocol         = "udp"
    port_range       = "7842"
    source_addresses = local.anywhere
  }

  # share edge iroh endpoint (edge config iroh.listen).
  inbound_rule {
    protocol         = "udp"
    port_range       = "7850"
    source_addresses = local.anywhere
  }

  # Path MTU discovery and ping; QUIC relies on PMTU.
  inbound_rule {
    protocol         = "icmp"
    source_addresses = local.anywhere
  }

  outbound_rule {
    protocol              = "tcp"
    port_range            = "1-65535"
    destination_addresses = local.anywhere
  }

  outbound_rule {
    protocol              = "udp"
    port_range            = "1-65535"
    destination_addresses = local.anywhere
  }

  outbound_rule {
    protocol              = "icmp"
    destination_addresses = local.anywhere
  }

  lifecycle {
    precondition {
      condition     = var.allow_ssh_from_anywhere || length(setintersection(toset(var.admin_cidrs), toset(local.anywhere))) == 0
      error_message = "admin_cidrs opens SSH to the internet; set allow_ssh_from_anywhere = true if that is intended."
    }
  }
}
# Pin everything to the DigitalOcean project explicitly (today it is the account's default project, renamed
# "theuth" on 2026-11-26), so a later change of the default project does not move or orphan the infra.
data "digitalocean_project" "this" {
  name = var.project
}

resource "digitalocean_project_resources" "this" {
  project = data.digitalocean_project.this.id
  resources = [
    digitalocean_droplet.host.urn,
    digitalocean_volume.data.urn,
    digitalocean_reserved_ip.public.urn,
  ]
}
