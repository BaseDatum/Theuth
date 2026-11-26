variable "dns_target_ip" {
  description = "Address every static A record points at. Default is the current home IP, so the first apply only imports. The switch is setting it to the droplet's reserved_ip."
  type        = string
  default     = "209.38.160.174"

  validation {
    condition     = can(cidrhost("${var.dns_target_ip}/32", 0))
    error_message = "dns_target_ip must be an IPv4 address."
  }
}

variable "dns_ttl" {
  description = "TTL of the static records in seconds. Porkbun's account minimum is normally 600; lower values are clamped or refused by the API."
  type        = number
  default     = 600
}

variable "record_ids" {
  description = "FQDN => Porkbun record id of the existing A record. Written by scripts/porkbun-import-ids.sh into import-ids.auto.tfvars.json."
  type        = map(string)
  default     = {}
}
