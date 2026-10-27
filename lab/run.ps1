# run.ps1 - runs the Nyx leak lab once (Windows). Builds the nyx-lab/daemon image from the repo
# root (cached after the first, long build), clears lab/out, runs every scenario in Docker Compose
# and tears the lab down. Exit code is the runner's: non-zero on any finding or broken scenario.
# Report: lab/out/report.md (plus pcaps and server logs in lab/out).
# Not 'Stop': Windows PowerShell 5.1 turns docker's progress output on stderr into a terminating
# error. Failures are checked through $LASTEXITCODE instead.
$ErrorActionPreference = 'Continue'
$lab = $PSScriptRoot
$root = Split-Path $lab -Parent
$compose = Join-Path $lab 'docker-compose.yml'

docker build -f (Join-Path $lab 'images/nyx-daemon.Dockerfile') -t nyx-lab/daemon $root
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# A stale capture or log would let the runner start too early or mix runs.
$out = Join-Path $lab 'out'
if (Test-Path $out) { Remove-Item -Recurse -Force (Join-Path $out '*') }
New-Item -ItemType Directory -Force $out | Out-Null

docker compose -f $compose up --build --abort-on-container-exit --exit-code-from runner
$code = $LASTEXITCODE
docker compose -f $compose down -v
exit $code
