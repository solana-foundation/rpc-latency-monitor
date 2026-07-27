terraform {
  required_version = ">= 1.5"
  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 5.0"
    }
  }
  # Remote state for CI. Local runs: `terraform init -backend=false`.
  # CI supplies bucket/prefix via -backend-config.
  backend "gcs" {}
}

provider "google" {
  project = var.project_id
}

resource "google_project_service" "compute" {
  service            = "compute.googleapis.com"
  disable_on_destroy = false
}

# Dedicated, minimally-privileged service account for the monitor VMs:
# only Artifact Registry read, instead of the default Compute SA (which
# often carries Project Editor). The cloud-platform scope is required for
# Artifact Registry pulls; the SA's IAM is what bounds the blast radius.
resource "google_service_account" "monitor" {
  account_id   = "rpc-latency-monitor"
  display_name = "rpc-latency-monitor VM"
}

resource "google_project_iam_member" "monitor_ar_reader" {
  project = var.project_id
  role    = "roles/artifactregistry.reader"
  member  = "serviceAccount:${google_service_account.monitor.email}"
}
