variable "instance_type" {
  type    = string
  default = "t3.small"
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
  default = "../../gcp/config.yaml"
}

variable "alloy_config_file" {
  type    = string
  default = "../../../grafana/alloy-config.alloy"
}
