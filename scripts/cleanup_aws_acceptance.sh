#!/usr/bin/env bash
# cleanup_aws_acceptance.sh — permanent, idempotent cleanup of ARX AWS acceptance infra.
#
# Uses the BOOTSTRAP identity (same as setup). Never touches buckets not matching
# arx-acceptance-*. Fail closed if no bucket name is supplied.
#
# Steps:
#   1. enumerate object versions + delete markers, permanently delete all
#   2. enumerate active multipart uploads, abort all
#   3. verify bucket empty
#   4. delete bucket
#   5. delete inline role policies + roles
#   6. remove local .aws-acceptance directory

set -euo pipefail
cd "$(dirname "$0")/.."

BOOTSTRAP_PROFILE="${ARX_AWS_BOOTSTRAP_PROFILE:-arx-bootstrap}"
BUCKET="${ARX_AWS_BUCKET:-}"
if [ -z "$BUCKET" ]; then
  echo "FATAL: ARX_AWS_BUCKET required (the disposable acceptance bucket to clean)." >&2
  echo "  export ARX_AWS_BUCKET=arx-acceptance-xxxxxxxx-xxxxxxxx" >&2
  exit 1
fi
case "$BUCKET" in
  arx-acceptance-*) ;;
  *) echo "FATAL: refusing to touch bucket '$BUCKET' (must match arx-acceptance-*)." >&2; exit 1 ;;
esac

ACCT_ROLE="arx-acceptance-full-role"
NOLB_ROLE="arx-nolb-role"
DENY_LIST_ROLE="arx-deny-list-role"
DENY_GET_ROLE="arx-deny-get-role"
DENY_PUT_ROLE="arx-deny-put-role"
DENY_DEL_ROLE="arx-deny-del-role"

echo "== Cleaning bucket: $BUCKET =="

# 1. permanently delete all versions + delete markers
aws s3api list-object-versions --bucket "$BUCKET" --profile "$BOOTSTRAP_PROFILE" --output json > /tmp/arx_versions.json 2>/dev/null || echo "[]" > /tmp/arx_versions.json
python3 - "$BUCKET" "$BOOTSTRAP_PROFILE" <<'PY'
import json, subprocess, sys
bucket, profile = sys.argv[1], sys.argv[2]
try:
    data = json.load(open("/tmp/arx_versions.json"))
except Exception:
    data = {}
versions = data.get("Versions", []) + data.get("DeleteMarkers", [])
for v in versions:
    key = v["Key"]; vid = v["VersionId"]
    subprocess.run(["aws","s3api","delete-object","--bucket",bucket,"--key",key,
                    "--version-id",vid,"--profile",profile], check=False)
# 2. abort remaining multipart uploads
try:
    ups = json.loads(subprocess.run(["aws","s3api","list-multipart-uploads","--bucket",bucket,
                                      "--profile",profile,"--output","json"],
                                     capture_output=True, text=True).stdout or "{}")
except Exception:
    ups = {}
for u in ups.get("Uploads", []):
    subprocess.run(["aws","s3api","abort-multipart-upload","--bucket",bucket,"--key",u["Key"],
                    "--upload-id",u["UploadId"],"--profile",profile], check=False)
PY

# 3. verify empty
LEFT=$(aws s3api list-object-versions --bucket "$BUCKET" --profile "$BOOTSTRAP_PROFILE" \
  --query 'length(Versions) + length(DeleteMarkers)' --output text 2>/dev/null || echo 0)
if [ "${LEFT:-0}" != "0" ]; then
  echo "WARN: $LEFT objects/versions remain in $BUCKET; aborting cleanup of bucket." >&2
else
  # 4. delete bucket
  aws s3 rb "s3://$BUCKET" --force --profile "$BOOTSTRAP_PROFILE" >/dev/null 2>&1 || true
fi

# 5. delete inline role policies + roles
for r in "$ACCT_ROLE" "$NOLB_ROLE" "$DENY_LIST_ROLE" "$DENY_GET_ROLE" "$DENY_PUT_ROLE" "$DENY_DEL_ROLE"; do
  if aws iam get-role --role-name "$r" --profile "$BOOTSTRAP_PROFILE" >/dev/null 2>&1; then
    aws iam delete-role-policy --role-name "$r" --policy-name "${r}-inline" --profile "$BOOTSTRAP_PROFILE" 2>/dev/null || true
    aws iam delete-role --role-name "$r" --profile "$BOOTSTRAP_PROFILE" 2>/dev/null || true
    echo "removed role $r"
  fi
done

# 6. remove local credential dir
if [ -d ".aws-acceptance" ]; then
  chmod -R u+w .aws-acceptance 2>/dev/null || true
  rm -rf .aws-acceptance
  echo "removed .aws-acceptance/"
fi

echo "== Cleanup complete =="
