#!/bin/bash
# Local PostgreSQL for demos and disposable test databases.
set -euo pipefail
cd "$(dirname "$0")/.."
mode=${1:-start}
shift || true
case "$mode" in
  start|stop) [[ $# == 0 ]] || { echo 'start/stop do not accept commands.' >&2; exit 1; } ;;
  test|demo|dev) [[ $# -gt 0 ]] || { echo 'Supply a command to run.' >&2; exit 1; } ;;
  *) echo 'Usage: scripts/postgres.sh start|stop|test COMMAND...|demo COMMAND...|dev COMMAND...' >&2; exit 1 ;;
esac

if [[ "$mode" == test && -n "${HONORUM_TEST_POSTGRES:-}" ]]; then
  exec "$@"
fi
if [[ "$mode" == demo || "$mode" == dev ]] && [[ -n "${HONORUM_DATABASE_URL:-}" ]]; then
  exec "$@"
fi

container_tool=${CONTAINER_TOOL:-}
if [[ -z "$container_tool" ]]; then
  if command -v podman >/dev/null; then container_tool=podman
  elif command -v docker >/dev/null; then container_tool=docker
  else echo 'Install Docker or Podman, or supply a PostgreSQL URL.' >&2; exit 1
  fi
fi
port=${HONORUM_LOCAL_POSTGRES_PORT:-15432}
[[ "$port" =~ ^[0-9]+$ ]] && ((port > 0 && port <= 65535)) || { echo 'Invalid local PostgreSQL port.' >&2; exit 1; }
container_name="honorum-postgres-local-$port"

if [[ "$mode" == stop ]]; then
  "$container_tool" stop "$container_name" >/dev/null
  exit
fi
if ! "$container_tool" container inspect "$container_name" >/dev/null 2>&1; then
  "$container_tool" run -d --name "$container_name" \
    -p "127.0.0.1:$port:5432" \
    -e POSTGRES_USER=honorum -e POSTGRES_PASSWORD=honorum-local \
    -v "$container_name:/var/lib/postgresql/data" \
    docker.io/library/postgres:17-alpine >/dev/null
else
  "$container_tool" start "$container_name" >/dev/null
fi
ready=0
for attempt in {1..60}; do
  if "$container_tool" exec "$container_name" pg_isready -h 127.0.0.1 -U honorum >/dev/null 2>&1; then ready=1; break; fi
  sleep 1
done
if [[ "$ready" != 1 ]]; then echo 'Local PostgreSQL did not become ready.' >&2; exit 1; fi

base="postgres://honorum:honorum-local@127.0.0.1:$port"
case "$mode" in
  start) ;;
  test)
    export HONORUM_TEST_POSTGRES="$base/postgres?sslmode=disable"
    exec "$@"
    ;;
  demo|dev)
    worktree_id=$(printf %s "$PWD" | cksum | cut -d' ' -f1)
    database="honorum_${mode}_$worktree_id"
    if [[ $("$container_tool" exec "$container_name" psql -U honorum -d postgres -Atc "SELECT 1 FROM pg_database WHERE datname='$database'") != 1 ]]; then
      "$container_tool" exec "$container_name" createdb -U honorum "$database"
    fi
    export HONORUM_DATABASE_URL="$base/$database?sslmode=disable"
    exec "$@"
    ;;
  *) echo 'Usage: scripts/postgres.sh start|stop|test COMMAND...|demo COMMAND...|dev COMMAND...' >&2; exit 1 ;;
esac
