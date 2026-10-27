# run.ps1 - runs the Nyx leak lab once (Windows). Builds the nyx-lab/daemon image from the repo
# root (cached after the first, long build), clears lab/out, runs every scenario in Docker Compose
# and tears the lab down. Exit code is the runner's: non-zero on any finding or broken scenario.
# Report: lab/out/report.md (plus pcaps and server logs in lab/out).
# The lab runs detached and the script follows the runner: scenarios stop and restart the client
# daemon and stop the capture sidecar, which `up --abort-on-container-exit` would treat as the
# end of the run.
# Not 'Stop': Windows PowerShell 5.1 turns docker's progress output on stderr into a terminating
# error. Failures are checked through $LASTEXITCODE instead.
$ErrorActionPreference = 'Continue'
$lab = $PSScriptRoot
$root = Split-Path $lab -Parent
$compose = Join-Path $lab 'docker-compose.yml'

docker build -f (Join-Path $lab 'images/nyx-daemon.Dockerfile') -t nyx-lab/daemon $root
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# A stale capture, log or volume from an aborted run would let the runner start too early or mix runs.
docker compose -f $compose down -v
$out = Join-Path $lab 'out'
if (Test-Path $out) { Remove-Item -Recurse -Force (Join-Path $out '*') }
New-Item -ItemType Directory -Force $out | Out-Null

docker compose -f $compose up -d --build
if ($LASTEXITCODE -ne 0) { docker compose -f $compose down -v; exit 1 }
docker logs -f nyx-lab-runner-1
$code = [int](docker wait nyx-lab-runner-1)
docker compose -f $compose down -v
exit $code
