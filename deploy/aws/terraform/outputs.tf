output "instance_ips" {
  value = {
    "us-east-1"      = module.us_east_1.public_ip
    "us-west-1"      = module.us_west_1.public_ip
    "eu-west-2"      = module.eu_west_2.public_ip
    "eu-central-1"   = module.eu_central_1.public_ip
    "eu-west-1"      = module.eu_west_1.public_ip
    "ap-northeast-1" = module.ap_northeast_1.public_ip
    "ap-southeast-1" = module.ap_southeast_1.public_ip
  }
}

output "monitor_image" {
  value = var.monitor_image
}
