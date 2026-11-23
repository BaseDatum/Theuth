# shellcheck shell=bash
# Sourced by the S3 scripts:  source "$ROOT/scripts/lib/s3-env.sh"
# Sets ENDPOINT (THEUTH_S3_ENDPOINT, default https://s3.rwx.dev, trailing slash stripped) and defines
#   s3_export_aws_env   rustfs/aws-cli env: path-style, no mandatory checksums, no IMDS probe
#                       (jarvis release-s3.sh:164-174). Caller decides when to export (package-linux: upload only).
#   s3_have_creds       true when AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY are both set; each caller keeps its own
#                       error wording and any dry-run/--check exemption.
ENDPOINT="${THEUTH_S3_ENDPOINT:-https://s3.rwx.dev}"; ENDPOINT="${ENDPOINT%/}"

s3_export_aws_env() {
  export AWS_S3_FORCE_PATH_STYLE=true
  export AWS_REQUEST_CHECKSUM_CALCULATION="${AWS_REQUEST_CHECKSUM_CALCULATION:-when_required}"
  export AWS_RESPONSE_CHECKSUM_VALIDATION="${AWS_RESPONSE_CHECKSUM_VALIDATION:-when_required}"
  export AWS_DEFAULT_REGION="${AWS_DEFAULT_REGION:-us-east-1}"
  export AWS_EC2_METADATA_DISABLED=true
}

s3_have_creds() { [[ -n "${AWS_ACCESS_KEY_ID:-}" && -n "${AWS_SECRET_ACCESS_KEY:-}" ]]; }
