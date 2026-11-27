variable "instance_type" {
  type    = string
  default = "t3.medium"
}

# Monthly on-demand list price per region for `instance_type`, USD, 730 h,
# compute only (20 GB gp3 root is ~$1.60/mo; the EIP is free while attached).
# Shown on the RPC Monitor Hosts board; replace with Cost Explorer figures.
variable "monthly_cost_usd" {
  type = map(number)
  default = {
    "us-east-1"      = 30.37
    "us-west-1"      = 36.21
    "eu-west-2"      = 34.46
    "eu-central-1"   = 35.04
    "eu-west-1"      = 33.29
    "ap-northeast-1" = 39.71
    "ap-southeast-1" = 38.54
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
  default = "../../gcp/config.yaml"
}

variable "alloy_config_file" {
  type    = string
  default = "../../../grafana/alloy-config.alloy"
}
