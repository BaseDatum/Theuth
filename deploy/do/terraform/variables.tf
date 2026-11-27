variable "name" {
  description = "Name of the droplet; prefixes the other resources."
  type        = string
  default     = "theuth"
}

variable "region" {
  description = "DigitalOcean region slug. The reserved IP and the volume are region-bound."
  type        = string
  default     = "lon1"
}

variable "size" {
  description = "Droplet size slug."
  type        = string
  default     = "s-1vcpu-1gb"
}

variable "image" {
  description = "Droplet image slug."
  type        = string
  default     = "ubuntu-24-04-x64"
}

variable "ssh_public_key_path" {
  description = "Public key uploaded as a digitalocean_ssh_key and installed for root."
  type        = string
  default     = "~/.ssh/id_ed25519.pub"
}

variable "admin_cidrs" {
  description = "CIDRs allowed to reach TCP 22. No default: set it (e.g. [\"203.0.113.7/32\"])."
  type        = list(string)

  validation {
    condition     = length(var.admin_cidrs) > 0 && alltrue([for c in var.admin_cidrs : can(cidrhost(c, 0))])
    error_message = "admin_cidrs must be a non-empty list of valid CIDRs."
  }
}

variable "allow_ssh_from_anywhere" {
  description = "Must be true for admin_cidrs to contain 0.0.0.0/0 or ::/0."
  type        = bool
  default     = false
}

variable "volume_size_gib" {
  description = "Size of the data volume mounted at /srv/theuth."
  type        = number
  default     = 10
}
variable "project" {
  description = "DigitalOcean project the droplet, volume and reserved IP belong to."
  type        = string
  default     = "theuth"
}
