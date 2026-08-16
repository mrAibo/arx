#!/usr/bin/env bash
# setup_aws_acceptance.sh — prepares ALL disposable AWS resources for ARX real-AWS acceptance.
#
# CREDENTIAL MODEL (correct per AWS guidance):
#   - Bootstrap identity (your configured profile, e.g. arx-bootstrap) is used ONLY to
#     create bucket/IAM infra and to assume test roles.
#   - We use `aws sts assume-role`, NOT `get-session-token` (the latter requires
#     long-lived IAM *user* keys, which we do not use).
#   - All ARX physical tests run with TEMPORARY assumed-role credentials.
#   - No long-lived access keys are created.
#   - IAM permissions use INLINE role policies (put-role-policy) — no customer-managed
#     policy version accumulation.
#
# OUTPUT: an isolated, gitignored directory `.aws-acceptance/` (mode 0700) containing
# temporary shared credentials + config files (mode 0600) with these profiles:
#   arx-full  arx-nolb  arx-deny-list  arx-deny-get  arx-deny-put  arx-deny-delete
# Each profile holds temporary AssumeRole credentials (access key / secret / session token).
#
# The script prints ONLY the env-var exports an operator sources. It NEVER prints
# secret values. Costs: Free Tier (new account) or fractions of a cent (existing).

set -euo pipefail
cd "$(dirname "$0")/.."

BOOTSTRAP_PROFILE="${ARX_AWS_BOOTSTRAP_PROFILE:-arx-bootstrap}"

# ── Punkt 2: fail closed on endpoint overrides (before ANY AWS call) ──
for ov in AWS_ENDPOINT_URL AWS_ENDPOINT_URL_S3; do
  val="${!ov:-}"
  if [ -n "${val// }" ]; then
    echo "FATAL: custom AWS endpoint override present ($ov=$val)." >&2
    echo "Unset $ov before real AWS acceptance:" >&2
    echo "  unset $ov" >&2
    exit 1
  fi
done
# Detect obvious localhost overrides in shared config (if readable).
if [ -n "${AWS_CONFIG_FILE:-}" ] && [ -r "${AWS_CONFIG_FILE:-}" ]; then
  if grep -qiE 'endpoint_url\s*=\s*(https?://(localhost|127\.0\.0\.1|minio|moto|localstack))' "${AWS_CONFIG_FILE}"; then
    echo "FATAL: shared config contains a localhost/minio/moto/localstack endpoint override." >&2
    echo "Remove it before real AWS acceptance." >&2
    exit 1
  fi
fi

# ── Punkt 7: required explicit trust principal ──
if [ -z "${ARX_AWS_TRUST_PRINCIPAL_ARN:-}" ]; then
  echo "FATAL: ARX_AWS_TRUST_PRINCIPAL_ARN is required (canonical bootstrap caller ARN)." >&2
  echo "Set it to the EXACT IAM user/role ARN that bootstraps acceptance, e.g.:" >&2
  echo "  export ARX_AWS_TRUST_PRINCIPAL_ARN=arn:aws:iam::123456789012:user/arx-bootstrap" >&2
  echo "We do NOT guess the canonical role ARN from an assumed-role ARN, and we do NOT use '*'." >&2
  exit 1
fi
TRUST_ARN="$ARX_AWS_TRUST_PRINCIPAL_ARN"

REGION="${ARX_AWS_REGION:-us-east-1}"
ACCT_ID=$(aws sts get-caller-identity --profile "$BOOTSTRAP_PROFILE" --query Account --output text)
RUN_RND=$(head -c 8 /dev/urandom | od -An -tx1 | tr -d ' \n')
# ── Punkt 8: stronger bucket name (account-id + random, not timestamp alone) ──
BUCKET="arx-acceptance-${ACCT_ID}-${RUN_RND}"
SESSION="${BUCKET}-session"

# Fail closed: bootstrap profile must authenticate.
if ! aws sts get-caller-identity --profile "$BOOTSTRAP_PROFILE" >/dev/null 2>&1; then
  echo "FATAL: bootstrap profile '$BOOTSTRAP_PROFILE' not authenticated. Run 'aws configure sso' / 'aws login' first." >&2
  exit 1
fi

ARN="arn:aws:s3:::$BUCKET"
ARN_OBJ="$ARN/arx-acceptance/*"

# ── Disposable bucket (fail closed; no silent reuse) ──
echo "== Creating disposable bucket: $BUCKET ($REGION) =="
if [ "$REGION" = "us-east-1" ]; then
  aws s3api create-bucket --bucket "$BUCKET" --region "$REGION" --profile "$BOOTSTRAP_PROFILE"
else
  aws s3api create-bucket --bucket "$BUCKET" --region "$REGION" \
    --create-bucket-configuration LocationConstraint="$REGION" --profile "$BOOTSTRAP_PROFILE"
fi
aws s3api put-bucket-versioning --bucket "$BUCKET" --versioning-configuration Status=Enabled --profile "$BOOTSTRAP_PROFILE"
# Public Access Block ON (acceptance hygiene).
aws s3api put-public-access-block --bucket "$BUCKET" \
  --public-access-block-configuration BlockPublicAcls=true,IgnorePublicAcls=true,BlockPublicPolicy=true,RestrictPublicBuckets=true \
  --profile "$BOOTSTRAP_PROFILE"

# ── Punkt 6: INLINE role policies (no managed-policy version leak) ──
mk_inline() { # $1=role $2=json-file
  aws iam put-role-policy --role-name "$1" --policy-name "${1}-inline" \
    --policy-document "file://$2" --profile "$BOOTSTRAP_PROFILE"
}
# ── Punkt 7: trust policy updated EVERY run (not only create-if-missing) ──
mk_role() { # $1=role-name
  if ! aws iam get-role --role-name "$1" --profile "$BOOTSTRAP_PROFILE" >/dev/null 2>&1; then
    aws iam create-role --role-name "$1" \
      --assume-role-policy-document "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\"Principal\":{\"AWS\":\"$TRUST_ARN\"},\"Action\":\"sts:AssumeRole\"}]}" \
      --profile "$BOOTSTRAP_PROFILE" >/dev/null
  else
    # refresh trust policy on every run
    aws iam update-assume-role-policy --role-name "$1" \
      --policy-document "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\"Principal\":{\"AWS\":\"$TRUST_ARN\"},\"Action\":\"sts:AssumeRole\"}]}" \
      --profile "$BOOTSTRAP_PROFILE"
  fi
  aws iam get-role --role-name "$1" --query 'Role.Arn' --output text --profile "$BOOTSTRAP_PROFILE"
}

ACCT_ROLE="arx-acceptance-full-role"
NOLB_ROLE="arx-nolb-role"
DENY_LIST_ROLE="arx-deny-list-role"
DENY_GET_ROLE="arx-deny-get-role"
DENY_PUT_ROLE="arx-deny-put-role"
DENY_DEL_ROLE="arx-deny-del-role"

cat > /tmp/arx_full.json <<EOF
{"Version":"2012-10-17","Statement":[
 {"Effect":"Allow","Action":["s3:ListAllMyBuckets"],"Resource":"*"},
 {"Effect":"Allow","Action":["s3:ListBucket","s3:GetBucketLocation","s3:GetBucketVersioning","s3:ListBucketMultipartUploads"],"Resource":"$ARN"},
 {"Effect":"Allow","Action":["s3:GetObject","s3:PutObject","s3:DeleteObject","s3:AbortMultipartUpload","s3:ListMultipartUploadParts"],"Resource":"$ARN_OBJ"}
]}
EOF

cat > /tmp/arx_nolb.json <<EOF
{"Version":"2012-10-17","Statement":[
 {"Effect":"Allow","Action":["s3:ListBucket","s3:GetBucketLocation","s3:GetBucketVersioning","s3:ListBucketMultipartUploads"],"Resource":"$ARN"},
 {"Effect":"Allow","Action":["s3:GetObject","s3:PutObject","s3:DeleteObject","s3:AbortMultipartUpload","s3:ListMultipartUploadParts"],"Resource":"$ARN_OBJ"}
]}
EOF

cat > /tmp/arx_dl.json <<EOF
{"Version":"2012-10-17","Statement":[
 {"Effect":"Allow","Action":["s3:GetBucketLocation","s3:GetBucketVersioning","s3:ListBucketMultipartUploads"],"Resource":"$ARN"},
 {"Effect":"Deny","Action":["s3:ListBucket"],"Resource":"$ARN"},
 {"Effect":"Allow","Action":["s3:GetObject","s3:PutObject","s3:DeleteObject","s3:AbortMultipartUpload","s3:ListMultipartUploadParts"],"Resource":"$ARN_OBJ"}
]}
EOF

cat > /tmp/arx_dg.json <<EOF
{"Version":"2012-10-17","Statement":[
 {"Effect":"Allow","Action":["s3:ListBucket","s3:GetBucketLocation"],"Resource":"$ARN"},
 {"Effect":"Allow","Action":["s3:PutObject","s3:DeleteObject","s3:AbortMultipartUpload","s3:ListMultipartUploadParts"],"Resource":"$ARN_OBJ"}
]}
EOF

cat > /tmp/arx_dp.json <<EOF
{"Version":"2012-10-17","Statement":[
 {"Effect":"Allow","Action":["s3:ListBucket","s3:GetBucketLocation"],"Resource":"$ARN"},
 {"Effect":"Allow","Action":["s3:GetObject","s3:DeleteObject","s3:AbortMultipartUpload","s3:ListMultipartUploadParts"],"Resource":"$ARN_OBJ"}
]}
EOF

cat > /tmp/arx_dd.json <<EOF
{"Version":"2012-10-17","Statement":[
 {"Effect":"Allow","Action":["s3:ListBucket","s3:GetBucketLocation"],"Resource":"$ARN"},
 {"Effect":"Allow","Action":["s3:GetObject","s3:PutObject","s3:AbortMultipartUpload","s3:ListMultipartUploadParts"],"Resource":"$ARN_OBJ"}
]}
EOF

FULL_ROLE_ARN=$(mk_role "$ACCT_ROLE");     mk_inline "$ACCT_ROLE"     /tmp/arx_full.json
NOLB_ROLE_ARN=$(mk_role "$NOLB_ROLE");     mk_inline "$NOLB_ROLE"     /tmp/arx_nolb.json
DL_ROLE_ARN=$(mk_role "$DENY_LIST_ROLE");  mk_inline "$DENY_LIST_ROLE" /tmp/arx_dl.json
DG_ROLE_ARN=$(mk_role "$DENY_GET_ROLE");   mk_inline "$DENY_GET_ROLE"  /tmp/arx_dg.json
DP_ROLE_ARN=$(mk_role "$DENY_PUT_ROLE");   mk_inline "$DENY_PUT_ROLE"  /tmp/arx_dp.json
DD_ROLE_ARN=$(mk_role "$DENY_DEL_ROLE");   mk_inline "$DENY_DEL_ROLE"  /tmp/arx_dd.json

# ── Assume roles -> temporary credentials ──
assume() { # $1=role-arn $2=session
  aws sts assume-role --role-arn "$1" --role-session-name "$2" --duration-seconds 3600 \
    --profile "$BOOTSTRAP_PROFILE" --output json
}
write_profile() { # $1=profile $2=creds-json
  local p="$1" j="$2"
  local ak sk tk
  ak=$(echo "$j" | python3 -c 'import json,sys;print(json.load(sys.stdin)["Credentials"]["AccessKeyId"])')
  sk=$(echo "$j" | python3 -c 'import json,sys;print(json.load(sys.stdin)["Credentials"]["SecretAccessKey"])')
  tk=$(echo "$j" | python3 -c 'import json,sys;print(json.load(sys.stdin)["Credentials"]["SessionToken"])')
  cat >> "$CREDS" <<EOF
[$p]
aws_access_key_id = $ak
aws_secret_access_key = $sk
aws_session_token = $tk
EOF
  cat >> "$CONF" <<EOF
[$p]
region = $REGION
EOF
}

# ── Isolated gitignored credential dir (Punkt 3: never mutate tracked .gitignore) ──
AWSDIR=".aws-acceptance"
mkdir -p "$AWSDIR"; chmod 700 "$AWSDIR"
CREDS="$AWSDIR/credentials"; CONF="$AWSDIR/config"
: > "$CREDS"; : > "$CONF"; chmod 600 "$CREDS" "$CONF"

write_profile "arx-full"      "$(assume "$FULL_ROLE_ARN" "${SESSION}-full")"
write_profile "arx-nolb"      "$(assume "$NOLB_ROLE_ARN" "${SESSION}-nolb")"
write_profile "arx-deny-list" "$(assume "$DL_ROLE_ARN"   "${SESSION}-dl")"
write_profile "arx-deny-get"  "$(assume "$DG_ROLE_ARN"   "${SESSION}-dg")"
write_profile "arx-deny-put"  "$(assume "$DP_ROLE_ARN"   "${SESSION}-dp")"
write_profile "arx-deny-delete" "$(assume "$DD_ROLE_ARN" "${SESSION}-dd")"

# ── Env file (NO global AWS_ACCESS_KEY_ID/SECRET/Token export) ──
ENV_FILE="$AWSDIR/env"
cat > "$ENV_FILE" <<EOF
# ARX AWS acceptance — disposable, temporary. Source this file. NEVER commit.
export AWS_SHARED_CREDENTIALS_FILE="$PWD/$CREDS"
export AWS_CONFIG_FILE="$PWD/$CONF"
export ARX_AWS_ACCEPTANCE=1
export ARX_AWS_BUCKET=$BUCKET
export ARX_AWS_REGION=$REGION
export ARX_AWS_FULL_PROFILE=arx-full
export ARX_AWS_NOLB_PROFILE=arx-nolb
export ARX_AWS_DENY_LIST_PROFILE=arx-deny-list
export ARX_AWS_DENY_GET_PROFILE=arx-deny-get
export ARX_AWS_DENY_PUT_PROFILE=arx-deny-put
export ARX_AWS_DENY_DEL_PROFILE=arx-deny-delete
EOF
chmod 600 "$ENV_FILE"

echo ""
echo "== Done. Source the env file, then run the physical tests: =="
echo "  source $ENV_FILE"
echo "  cargo test --locked --all-features --test s3_acc_aws -- --nocapture"
echo ""
echo "Roles: $ACCT_ROLE $NOLB_ROLE $DENY_LIST_ROLE $DENY_GET_ROLE $DENY_PUT_ROLE $DENY_DEL_ROLE"
echo "Cleanup after: bash scripts/cleanup_aws_acceptance.sh"
