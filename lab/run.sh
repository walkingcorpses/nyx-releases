#!/bin/sh
# run.sh - runs the Nyx leak lab once (Linux/macOS/Git Bash). Same steps as run.ps1: build the
# nyx-lab/daemon image from the repo root, clear lab/out, run the scenarios (detached, following
# the runner's output), tear down. Exits with the runner's code (non-zero on any finding or broken
# scenario). Report: lab/out/report.md.
set -eu
lab=$(cd "$(dirname "$0")" && pwd)
root=$(dirname "$lab")
compose="$lab/docker-compose.yml"
docker build -f "$lab/images/nyx-daemon.Dockerfile" -t nyx-lab/daemon "$root"
# A stale capture, log or volume from an aborted run would let the runner start too early or mix runs.
docker compose -f "$compose" down -v
rm -rf "$lab/out"
mkdir -p "$lab/out"
if ! docker compose -f "$compose" up -d --build; then
    docker compose -f "$compose" down -v
    exit 1
fi
docker logs -f nyx-lab-runner-1 || true
code=$(docker wait nyx-lab-runner-1)
docker compose -f "$compose" down -v
exit "$code"
