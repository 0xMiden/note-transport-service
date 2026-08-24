#!/usr/bin/env bash

set -euo pipefail

if [[ -n "${MNT_TEST_POSTGRES_URL:-}" ]]; then
    BUILD_PROTO=1 cargo nextest run --workspace
    exit
fi

container="mnt-postgres-test-$$"
cleanup() {
    docker rm --force "$container" >/dev/null 2>&1 || true
}
trap cleanup EXIT

docker run \
    --detach \
    --name "$container" \
    --env POSTGRES_DB=note_transport \
    --env POSTGRES_PASSWORD=postgres \
    --publish 127.0.0.1::5432 \
    postgres:17-alpine >/dev/null

ready=false
for _ in {1..30}; do
    if docker exec "$container" pg_isready --dbname note_transport --username postgres >/dev/null
    then
        ready=true
        break
    fi
    sleep 1
done

if [[ "$ready" != true ]]; then
    echo "PostgreSQL did not become ready" >&2
    exit 1
fi

mapping="$(docker port "$container" 5432/tcp)"
port="${mapping##*:}"
MNT_TEST_POSTGRES_URL="postgres://postgres:postgres@127.0.0.1:${port}/note_transport" \
    BUILD_PROTO=1 cargo nextest run --workspace
