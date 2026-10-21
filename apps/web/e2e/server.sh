#!/bin/sh
set -eu
cd "$(dirname "$0")/../../.."
if [ -z "${HONORUM_TEST_POSTGRES:-}" ]; then
  exec ./scripts/postgres.sh test sh apps/web/e2e/server.sh
fi
unset HONORUM_BASE_URL HONORUM_VAPID_PUBLIC HONORUM_VAPID_PRIVATE HONORUM_CANVAS_KEY
unset HONORUM_CATALOG_OBJECTS HONORUM_ROLE
unset HONORUM_TWILIO_ACCOUNT_SID HONORUM_TWILIO_AUTH_TOKEN HONORUM_TWILIO_VERIFY_SERVICE_SID
cargo build --release --locked -p honorum
cargo build --release --locked -p honorum --features lab --bin honorum-lab
target=${CARGO_TARGET_DIR:-target}
HONORUM_E2E_BINARY="$target/release/honorum" "$target/release/honorum-lab" browser &
test_pid=$!
trap 'kill -TERM "$test_pid" 2>/dev/null || true; wait "$test_pid" || true' EXIT INT TERM
wait "$test_pid"
