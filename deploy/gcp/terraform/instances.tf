resource "google_compute_instance" "monitor" {
  for_each     = var.locations
  name         = "rpc-latency-monitor-${each.key}"
  machine_type = var.machine_type
  zone         = each.value
  depends_on   = [google_project_service.compute, google_project_iam_member.monitor_ar_reader]

  boot_disk {
    initialize_params {
      image = "projects/cos-cloud/global/images/family/cos-stable"
      size  = 20
    }
  }

  network_interface {
    network = "default"
    access_config {}
  }

  service_account {
    email  = google_service_account.monitor.email
    scopes = ["cloud-platform"]
  }

  metadata = {
    monitor-region = each.key
    monitor-image  = var.monitor_image
    doppler-token  = var.doppler_token
    monitor-config = file(var.config_file)
    alloy-config   = file(var.alloy_config_file)
    startup-script = file("${path.module}/../startup-script.sh")
  }

  labels = {
    service = "rpc-latency-monitor"
    region  = each.key
  }
}
