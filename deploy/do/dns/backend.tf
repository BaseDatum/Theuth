# State lives in Wasabi (S3-compatible), bucket theuth-tf-state in us-central-1.
# Credentials from env only: AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY (Bitwarden "Wasabi theuth (us-central-1)",
# username = access key id, password = secret; vault wasabi.theuth.*).
terraform {
  backend "s3" {
    bucket = "theuth-tf-state"
    key    = "deploy/do/dns.tfstate"
    region = "us-central-1"
    endpoints = {
      s3 = "https://s3.us-central-1.wasabisys.com"
    }
    use_path_style              = true
    skip_credentials_validation = true
    skip_region_validation      = true
    skip_requesting_account_id  = true
    skip_metadata_api_check     = true
    skip_s3_checksum            = true
    use_lockfile                = true
  }
}
