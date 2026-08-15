#!/usr/bin/env bash
# S3-77: MinIO physical acceptance demo for ARX.
#
# Requires a disposable MinIO. If one is not already running, this script can
# start one via Docker (commented). It then runs the env-gated MinIO acceptance
# tests against the live endpoint using the SAME production S3Provider/runtime.
#
# This is PHYSICAL PASS for MinIO — it is NOT an AWS SUPPORTED claim. AWS real
# physical acceptance (S3-62A..65A) remains PARKED_ENV (no disposable AWS).

set -euo pipefail

MINIO_CONTAINER="${ARX_MINIO_CONTAINER:-arx-minio-test}"
MINIO_BUCKET="${ARX_MINIO_BUCKET:-arxtest}"
MINIO_ENDPOINT="${ARX_MINIO_ENDPOINT:-http://localhost:9000}"
MINIO_ROOT_USER="${ARX_MINIO_ROOT_USER:-minioadmin}"
MINIO_ROOT_PASSWORD="${ARX_MINIO_ROOT_PASSWORD:-minioadmin}"

# --- optional: start a disposable MinIO if not running ---
if ! docker ps --format '{{.Names}}' | grep -q "^${MINIO_CONTAINER}$"; then
  echo ">> starting disposable MinIO (${MINIO_CONTAINER})"
  docker run -d --name "${MINIO_CONTAINER}" -p 9000:9000 -p 9001:9001 \
    -e "MINIO_ROOT_USER=${MINIO_ROOT_USER}" \
    -e "MINIO_ROOT_PASSWORD=${MINIO_ROOT_PASSWORD}" \
    minio/minio server /data --console-address ":9001" >/dev/null
  # create the demo bucket
  docker run --rm --network host rclone/rclone \
    rclone mkdir ":s3:${MINIO_BUCKET}" \
    --s3-endpoint "${MINIO_ENDPOINT}" \
    --s3-access-key-id "${MINIO_ROOT_USER}" \
    --s3-secret-access-key "${MINIO_ROOT_PASSWORD}" \
    --s3-provider Minio || true
fi

echo ">> running ARX MinIO physical acceptance (S3-66/67)"
export ARX_MINIO_TEST=1
export AWS_ENDPOINT_URL="${MINIO_ENDPOINT}"
export AWS_ACCESS_KEY_ID="${MINIO_ROOT_USER}"
export AWS_SECRET_ACCESS_KEY="${MINIO_ROOT_PASSWORD}"

# The ARX S3 target config points at this endpoint + force_path_style.
# Acceptance tests use the SAME S3Provider as production — no MinIO-special path.
cargo test --test s3_acc_minio -- --nocapture

echo ">> MinIO physical acceptance: PHYSICAL PASS (not an AWS SUPPORTED claim)"
