# Static A records of theuth.io and tun.zip at Porkbun (separate root module and state from the droplet, so a
# droplet rebuild never touches DNS).
#
# Every record here already exists at Porkbun: it is IMPORTED (ids from scripts/porkbun-import-ids.sh, written to
# import-ids.auto.tfvars.json) and then edited in place. A record whose id is not in var.record_ids is not managed
# at all, so this module can never create a duplicate A record. prevent_destroy blocks deletes and replacements.
#
# NOT managed, on purpose: the per-name `<n>.tun.zip` / `*.<n>.tun.zip` CNAMEs and `_acme-challenge` TXTs the share
# edge writes itself, the tun.zip CAA records (they pin the edge's ACME account), and every non-A record.
#
# Credentials from the environment only: PORKBUN_API_KEY / PORKBUN_SECRET_KEY (the porkbun.tunzip.* key, which is
# allowed on both zones).

terraform {
  required_version = ">= 1.7"

  required_providers {
    porkbun = {
      source  = "jianyuan/porkbun"
      version = "~> 0.4"
    }
  }
}

provider "porkbun" {}

locals {
  # key = FQDN as Porkbun reports it in `name`; subdomain null = zone apex.
  static_records = {
    "theuth.io"            = { domain = "theuth.io", subdomain = null }
    "www.theuth.io"        = { domain = "theuth.io", subdomain = "www" }
    "relay.theuth.io"      = { domain = "theuth.io", subdomain = "relay" }
    "app.theuth.io"        = { domain = "theuth.io", subdomain = "app" }
    "iroh.theuth.io"       = { domain = "theuth.io", subdomain = "iroh" }
    "iroh.relay.theuth.io" = { domain = "theuth.io", subdomain = "iroh.relay" }
    "*.theuth.io"          = { domain = "theuth.io", subdomain = "*" }
    "tun.zip"              = { domain = "tun.zip", subdomain = null }
    "*.tun.zip"            = { domain = "tun.zip", subdomain = "*" }
    "edge.tun.zip"         = { domain = "tun.zip", subdomain = "edge" }
    "iroh.tun.zip"         = { domain = "tun.zip", subdomain = "iroh" }
    "relay.tun.zip"        = { domain = "tun.zip", subdomain = "relay" }
  }

  managed = { for k, v in local.static_records : k => v if contains(keys(var.record_ids), k) }
  absent  = sort([for k in keys(local.static_records) : k if !contains(keys(var.record_ids), k)])
  unknown = sort([for k in keys(var.record_ids) : k if !contains(keys(local.static_records), k)])
}

import {
  for_each = local.managed
  to       = porkbun_dns_record.a[each.key]
  id       = "${var.record_ids[each.key]}_${each.value.domain}_A"
}

resource "porkbun_dns_record" "a" {
  for_each = local.managed

  domain    = each.value.domain
  subdomain = each.value.subdomain
  type      = "A"
  content   = var.dns_target_ip
  ttl       = var.dns_ttl

  lifecycle {
    prevent_destroy = true
  }
}

check "record_inventory" {
  assert {
    condition     = length(local.unknown) == 0
    error_message = "record_ids has keys that are not static records here: ${join(", ", local.unknown)}"
  }
}

output "managed" {
  value = { for k, r in porkbun_dns_record.a : k => "${r.content} ttl=${r.ttl}" }
}

output "not_present_at_porkbun" {
  description = "Static names without their own A record (served by a wildcard or not in the zone); not managed."
  value       = local.absent
}
