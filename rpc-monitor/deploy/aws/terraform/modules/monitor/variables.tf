variable "region_label" {
  type = string
}

variable "instance_type" {
  type = string
}

variable "instance_profile" {
  type = string
}

variable "monitor_image" {
  type = string
}

variable "doppler_token" {
  type      = string
  sensitive = true
}

variable "monitor_config" {
  type = string
}

variable "alloy_config" {
  type = string
}
