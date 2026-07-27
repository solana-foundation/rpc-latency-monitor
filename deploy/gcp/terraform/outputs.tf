output "instance_ips" {
  value = {
    for key, instance in google_compute_instance.monitor :
    key => instance.network_interface[0].access_config[0].nat_ip
  }
}

output "monitor_image" {
  value = var.monitor_image
}
