#!/bin/sh
# run.sh - runs the Nyx leak lab once (Linux/macOS/Git Bash). Same steps as run.ps1: build the
# nyx-lab/daemon image from the repo root, clear lab/out, run the scenarios, tear down. Exits with
# the runner's code (non-zero on any finding or broken scenario). Report: lab/out/report.md.
set -eu
lab=$(cd "$(dirname "$0")" && pwd)
root=$(dirname "$lab")
docker build -f "$lab/images/nyx-daemon.Dockerfile" -t nyx-lab/daemon "$root"
# A stale capture or log would let the runner start too early or mix runs.
rm -rf "$lab/out"
mkdir -p "$lab/out"
code=0
docker compose -f "$lab/docker-compose.yml" up --build --abort-on-container-exit --exit-code-from runner || code=$?
docker compose -f "$lab/docker-compose.yml" down -v
exit "$code"
