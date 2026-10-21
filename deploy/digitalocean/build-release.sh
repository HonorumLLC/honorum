#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/../.."
test -z "$(git status --porcelain)" || { echo 'Commit the release source before building.' >&2; exit 1; }
release_sha=$(git rev-parse HEAD)
container_tool=${CONTAINER_TOOL:-podman}
release_dir="artifacts/production/releases/$release_sha"
mkdir -p "$release_dir"
"$container_tool" build --build-arg "VERSION=$release_sha" -f deploy/digitalocean/Dockerfile -t "localhost/honorum:$release_sha" .
"$container_tool" save "localhost/honorum:$release_sha" | gzip > "$release_dir/image.tar.gz"
cp deploy/digitalocean/compose.yaml deploy/digitalocean/honorum.service deploy/digitalocean/install-release.sh "$release_dir/"
printf '%s\n' "$release_sha" > "$release_dir/version"
(cd "$release_dir" && sha256sum image.tar.gz compose.yaml honorum.service install-release.sh version > SHA256SUMS)
tar -czf "$release_dir.tar.gz" -C "$release_dir" .
echo "$release_dir.tar.gz"
