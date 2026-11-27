locals {
  monitor_config = file(var.config_file)
  alloy_config   = file(var.alloy_config_file)

  common = {
    instance_type    = var.instance_type
    cost_source      = var.cost_source
    instance_profile = aws_iam_instance_profile.monitor.name
    monitor_image    = var.monitor_image
    doppler_token    = var.doppler_token
    monitor_config   = local.monitor_config
    alloy_config     = local.alloy_config
  }
}

module "us_east_1" {
  source           = "./modules/monitor"
  providers        = { aws = aws.us_east_1 }
  region_label     = "us-east-1"
  monthly_cost_usd = var.monthly_cost_usd["us-east-1"]

  instance_type    = local.common.instance_type
  cost_source      = local.common.cost_source
  instance_profile = local.common.instance_profile
  monitor_image    = local.common.monitor_image
  doppler_token    = local.common.doppler_token
  monitor_config   = local.common.monitor_config
  alloy_config     = local.common.alloy_config
}

module "us_west_1" {
  source           = "./modules/monitor"
  providers        = { aws = aws.us_west_1 }
  region_label     = "us-west-1"
  monthly_cost_usd = var.monthly_cost_usd["us-west-1"]

  instance_type    = local.common.instance_type
  cost_source      = local.common.cost_source
  instance_profile = local.common.instance_profile
  monitor_image    = local.common.monitor_image
  doppler_token    = local.common.doppler_token
  monitor_config   = local.common.monitor_config
  alloy_config     = local.common.alloy_config
}

module "eu_west_2" {
  source           = "./modules/monitor"
  providers        = { aws = aws.eu_west_2 }
  region_label     = "eu-west-2"
  monthly_cost_usd = var.monthly_cost_usd["eu-west-2"]

  instance_type    = local.common.instance_type
  cost_source      = local.common.cost_source
  instance_profile = local.common.instance_profile
  monitor_image    = local.common.monitor_image
  doppler_token    = local.common.doppler_token
  monitor_config   = local.common.monitor_config
  alloy_config     = local.common.alloy_config
}

module "eu_central_1" {
  source           = "./modules/monitor"
  providers        = { aws = aws.eu_central_1 }
  region_label     = "eu-central-1"
  monthly_cost_usd = var.monthly_cost_usd["eu-central-1"]

  instance_type    = local.common.instance_type
  cost_source      = local.common.cost_source
  instance_profile = local.common.instance_profile
  monitor_image    = local.common.monitor_image
  doppler_token    = local.common.doppler_token
  monitor_config   = local.common.monitor_config
  alloy_config     = local.common.alloy_config
}

module "eu_west_1" {
  source           = "./modules/monitor"
  providers        = { aws = aws.eu_west_1 }
  region_label     = "eu-west-1"
  monthly_cost_usd = var.monthly_cost_usd["eu-west-1"]

  instance_type    = local.common.instance_type
  cost_source      = local.common.cost_source
  instance_profile = local.common.instance_profile
  monitor_image    = local.common.monitor_image
  doppler_token    = local.common.doppler_token
  monitor_config   = local.common.monitor_config
  alloy_config     = local.common.alloy_config
}

module "ap_northeast_1" {
  source           = "./modules/monitor"
  providers        = { aws = aws.ap_northeast_1 }
  region_label     = "ap-northeast-1"
  monthly_cost_usd = var.monthly_cost_usd["ap-northeast-1"]

  instance_type    = local.common.instance_type
  cost_source      = local.common.cost_source
  instance_profile = local.common.instance_profile
  monitor_image    = local.common.monitor_image
  doppler_token    = local.common.doppler_token
  monitor_config   = local.common.monitor_config
  alloy_config     = local.common.alloy_config
}

module "ap_southeast_1" {
  source           = "./modules/monitor"
  providers        = { aws = aws.ap_southeast_1 }
  region_label     = "ap-southeast-1"
  monthly_cost_usd = var.monthly_cost_usd["ap-southeast-1"]

  instance_type    = local.common.instance_type
  cost_source      = local.common.cost_source
  instance_profile = local.common.instance_profile
  monitor_image    = local.common.monitor_image
  doppler_token    = local.common.doppler_token
  monitor_config   = local.common.monitor_config
  alloy_config     = local.common.alloy_config
}
