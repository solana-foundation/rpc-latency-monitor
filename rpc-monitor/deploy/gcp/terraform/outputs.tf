output "instance_ips" {
  value = {
    for key, instance in google_compute_instance.monitor :
    key => instance.network_interface[0].access_config[0].nat_ip
  }
}

output "monitor_image" {
  value = var.monitor_image
}

# name => zone for every instance terraform manages; deploy.sh resets exactly
# these, so a hand-made box in the project is never rolled by a deploy.
output "monitor_instances" {
  value = {
    for key, instance in google_compute_instance.monitor :
    instance.name => instance.zone
  }
}
