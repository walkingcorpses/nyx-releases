# build-release.ps1 - builds the single portable nyx.exe.
#   1. builds the Svelte UI (ui\dist), which the exe embeds
#   2. builds the exe in release mode with the `custom-protocol` feature (UI served from inside the exe)
#      - uses `cargo tauri build --no-bundle` when the Tauri CLI is installed, plain cargo otherwise;
#        both run the same cargo build, the CLI just adds its own bookkeeping
#   3. copies it to dist\nyx.exe, then prints its size and the DLLs it depends on
# Run from anywhere:  powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

# --- UI -------------------------------------------------------------------
if (-not (Test-Path 'ui\node_modules')) { npm --prefix ui install --no-audit --no-fund; if ($LASTEXITCODE) { throw 'npm install failed' } }
npm --prefix ui run build
if ($LASTEXITCODE) { throw 'UI build failed' }

# --- exe ------------------------------------------------------------------
if (Get-Command cargo-tauri -ErrorAction SilentlyContinue) {
    cargo tauri build --no-bundle
} else {
    cargo build --release -p nyx --features custom-protocol
}
if ($LASTEXITCODE) { throw 'cargo build failed' }

New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item target\release\nyx.exe dist\nyx.exe -Force
$exe = Get-Item dist\nyx.exe
$mb = [math]::Round($exe.Length / 1MB, 1)
Write-Host "dist\nyx.exe: $mb MB"
if ($mb -ge 30) { Write-Warning 'exe is over the 30 MB target' }

# --- dependency check -----------------------------------------------------
# dumpbin lives in the VS Build Tools; find the newest one.
$dumpbin = Get-ChildItem 'C:\Program Files*\Microsoft Visual Studio\*\*\VC\Tools\MSVC\*\bin\HostX64\x64\dumpbin.exe' -ErrorAction SilentlyContinue |
    Sort-Object FullName | Select-Object -Last 1
if (-not $dumpbin) { Write-Warning 'dumpbin not found; skipping the dependency check'; exit 0 }
$deps = & $dumpbin.FullName /dependents dist\nyx.exe | Select-String '\.dll' | ForEach-Object { $_.Line.Trim() } | Sort-Object -Unique
Write-Host "DLL dependencies ($($deps.Count)):"
$deps | ForEach-Object { Write-Host "  $_" }
# Anything that is not a Windows system DLL is a packaging problem.
$bad = $deps | Where-Object { $_ -match 'torrent|ssl|crypto|boost|WebView2Loader|vcruntime|msvcp' }
if ($bad) { throw "unexpected non-system DLLs: $($bad -join ', ')" }
Write-Host 'OK: only system DLLs.'
