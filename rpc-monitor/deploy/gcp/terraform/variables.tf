variable "project_id" {
  type = string
}

variable "locations" {
  type = map(string)
  default = {
    "us-east4"        = "us-east4-b"
    "us-west2"        = "us-west2-b"
    "europe-west2"    = "europe-west2-b"
    "europe-west3"    = "europe-west3-b"
    "europe-west4"    = "europe-west4-a"
    "asia-northeast1" = "asia-northeast1-b"
    "asia-southeast1" = "asia-southeast1-b"
  }
}

variable "machine_type" {
  type    = string
  default = "e2-medium"
}

# Monthly list price per region for `machine_type`, USD, 730 h, on-demand,
# compute only (the 20 GB boot disk is < $1/mo). Shown on the RPC Monitor Hosts
# board; replace with billing-export figures when they are available.
variable "monthly_cost_usd" {
  type = map(number)
  default = {
    "us-east4"        = 27.55
    "us-west2"        = 29.42
    "europe-west2"    = 31.55
    "europe-west3"    = 31.55
    "europe-west4"    = 26.97
    "asia-northeast1" = 31.40
    "asia-southeast1" = 30.19
  }
}

# Set to "invoice" once monthly_cost_usd holds billed figures rather than list prices.
variable "cost_source" {
  type    = string
  default = "list-price"
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
  default = "../config.yaml"
}

variable "alloy_config_file" {
  type    = string
  default = "../../../grafana/alloy-config.alloy"
}
