data "aws_vpc" "default" {
  default = true
}

data "aws_ami" "al2023" {
  most_recent = true
  owners      = ["amazon"]
  filter {
    name   = "name"
    values = ["al2023-ami-2023.*-kernel-*-x86_64"]
  }
}

resource "aws_security_group" "monitor" {
  name        = "rpc-latency-monitor"
  description = "rpc-latency-monitor egress only"
  vpc_id      = data.aws_vpc.default.id

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = {
    service = "rpc-latency-monitor"
  }
}

resource "aws_instance" "monitor" {
  ami                         = data.aws_ami.al2023.id
  instance_type               = var.instance_type
  vpc_security_group_ids      = [aws_security_group.monitor.id]
  iam_instance_profile        = var.instance_profile
  user_data_replace_on_change = true

  credit_specification {
    cpu_credits = "unlimited"
  }

  user_data = "${templatefile("${path.module}/user-data.sh", {
    monitor_image      = var.monitor_image
    monitor_region     = var.region_label
    doppler_token      = var.doppler_token
    monitor_config_b64 = base64encode(var.monitor_config)
    alloy_config_b64   = base64encode(var.alloy_config)
  })}\n${file("${path.module}/../../../../shared/run-monitor.sh")}"

  metadata_options {
    http_tokens                 = "required"
    http_put_response_hop_limit = 1
    http_endpoint               = "enabled"
  }

  root_block_device {
    volume_size = 20
  }

  tags = {
    Name    = "rpc-latency-monitor-${var.region_label}"
    service = "rpc-latency-monitor"
    region  = var.region_label
  }
}

resource "aws_eip" "monitor" {
  instance = aws_instance.monitor.id
  domain   = "vpc"

  tags = {
    Name    = "rpc-latency-monitor-${var.region_label}"
    service = "rpc-latency-monitor"
    region  = var.region_label
  }
}
