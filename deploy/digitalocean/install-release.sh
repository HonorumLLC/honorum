#!/bin/bash
# Run as root only after transferring a reviewed bundle over verified SSH.
set -euo pipefail
test "$(id -u)" = 0
cd "$(dirname "$0")"
sha256sum -c SHA256SUMS
release_sha=$(cat version)
[[ "$release_sha" =~ ^[a-f0-9]{40}$ ]]
test -s /etc/honorum/app.env
test -s /etc/honorum/postgres-ca.crt
test -s /etc/honorum/tunnel-token
grep -q '^HONORUM_DATABASE_URL=postgres' /etc/honorum/app.env
gzip -dc image.tar.gz | docker load
previous=''
if [ -L /opt/honorum/current ]; then previous=$(readlink -f /opt/honorum/current); fi
if [ -n "$previous" ] && [ "$previous" != "$PWD" ]; then
  ln -sfn "$previous" /opt/honorum/previous
fi
printf 'HONORUM_IMAGE=localhost/honorum:%s\n' "$release_sha" > /etc/honorum/release.env
chmod 600 /etc/honorum/release.env
ln -sfn "$PWD" /opt/honorum/current
install -m 0644 honorum.service /etc/systemd/system/honorum.service
systemctl daemon-reload
systemctl enable honorum.service
systemctl restart honorum.service
docker compose --env-file /etc/honorum/release.env ps
