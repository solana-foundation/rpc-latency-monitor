variable "project_id" {
  type = string
}

variable "locations" {
  type = map(string)
  default = {
    "us-east1"        = "us-east1-b"
    "europe-west3"    = "europe-west3-b"
    "asia-northeast1" = "asia-northeast1-b"
  }
}

variable "machine_type" {
  type    = string
  default = "e2-small"
}

variable "monitor_image" {
  type = string
}

variable "doppler_token" {
  type      = string
  sensitive = true
}

variable "config_file" {
  type    = string
  default = "../../../config.yaml"
}

variable "alloy_config_file" {
  type    = string
  default = "../../../grafana/alloy-config.alloy"
}
