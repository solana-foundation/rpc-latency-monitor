terraform {
  required_version = ">= 1.5"
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }
  # Remote state for CI. Local runs: init with -backend-config for bucket/region.
  backend "s3" {}
}

provider "aws" {
  alias   = "us_east_1"
  region  = "us-east-1"
}

provider "aws" {
  alias   = "us_west_1"
  region  = "us-west-1"
}

provider "aws" {
  alias   = "eu_west_2"
  region  = "eu-west-2"
}

provider "aws" {
  alias   = "eu_central_1"
  region  = "eu-central-1"
}

provider "aws" {
  alias   = "eu_west_1"
  region  = "eu-west-1"
}

provider "aws" {
  alias   = "ap_northeast_1"
  region  = "ap-northeast-1"
}

provider "aws" {
  alias   = "ap_southeast_1"
  region  = "ap-southeast-1"
}

resource "aws_iam_role" "monitor" {
  provider = aws.us_east_1
  name     = "rpc-latency-monitor"
  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Action    = "sts:AssumeRole"
      Effect    = "Allow"
      Principal = { Service = "ec2.amazonaws.com" }
    }]
  })
}

resource "aws_iam_role_policy_attachment" "ssm" {
  provider   = aws.us_east_1
  role       = aws_iam_role.monitor.name
  policy_arn = "arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore"
}

resource "aws_iam_instance_profile" "monitor" {
  provider = aws.us_east_1
  name     = "rpc-latency-monitor"
  role     = aws_iam_role.monitor.name
}
