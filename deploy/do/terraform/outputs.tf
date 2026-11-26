output "reserved_ip" {
  description = "The public address every DNS record points at after the switch."
  value       = digitalocean_reserved_ip.public.ip_address
}

output "droplet_ip" {
  description = "The droplet's own address. After cloud-init moves the default route to the anchor gateway, use reserved_ip."
  value       = digitalocean_droplet.host.ipv4_address
}

output "ssh" {
  description = "SSH command (root, key from ssh_public_key_path)."
  value       = "ssh root@${digitalocean_reserved_ip.public.ip_address}"
}

output "volume_id" {
  value = digitalocean_volume.data.id
}
