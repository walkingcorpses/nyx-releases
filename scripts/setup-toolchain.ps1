# setup-toolchain.ps1
# Installs everything needed to build Nyx on Windows x64:
#   - Rust (rustup, stable-x86_64-pc-windows-msvc)
#   - Visual Studio 2022 Build Tools, "Desktop development with C++" workload
#   - CMake
#   - vcpkg (cloned to D:\Projects\tools\vcpkg, VCPKG_ROOT persisted for the user)
#   - libtorrent:x64-windows-static via vcpkg (static libs, static CRT)
# Each step is skipped when the tool is already present, so re-running is safe.
# Run from any PowerShell window: powershell -ExecutionPolicy Bypass -File scripts\setup-toolchain.ps1
# The Build Tools install may raise a UAC prompt.

$ErrorActionPreference = 'Stop'
$VcpkgRoot = if ($env:VCPKG_ROOT) { $env:VCPKG_ROOT } else { 'D:\Projects\tools\vcpkg' }

# Prints a step banner so the log shows what is being installed and why.
function Step($msg) { Write-Host "==> $msg" -ForegroundColor Cyan }

# True when a command resolves on PATH (or at an explicit path).
function Have($cmd) { [bool](Get-Command $cmd -ErrorAction SilentlyContinue) }

# 1. Rust via rustup-init from the official rustup site.
$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
if (-not (Test-Path (Join-Path $cargoBin 'rustup.exe'))) {
    Step 'Installing rustup (stable-x86_64-pc-windows-msvc)'
    $init = Join-Path $env:TEMP 'rustup-init.exe'
    Invoke-WebRequest 'https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe' -OutFile $init
    & $init -y --default-toolchain stable-x86_64-pc-windows-msvc --profile default
    if ($LASTEXITCODE) { throw 'rustup-init failed' }
} else { Step 'rustup already installed, skipping' }
$env:PATH = "$cargoBin;$env:PATH"

# 2. Visual Studio Build Tools with the C++ workload (MSVC compiler, Windows SDK).
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$hasVc = (Test-Path $vswhere) -and (& $vswhere -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath)
if (-not $hasVc) {
    Step 'Installing Visual Studio 2022 Build Tools (Desktop development with C++)'
    winget install --id Microsoft.VisualStudio.2022.BuildTools -e --accept-package-agreements --accept-source-agreements `
        --override '--quiet --wait --norestart --nocache --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended'
    if ($LASTEXITCODE) { throw "Build Tools install failed ($LASTEXITCODE)" }
} else { Step 'MSVC build tools already installed, skipping' }

# 3. CMake (vcpkg can fetch its own, but later tooling expects it on PATH).
if (-not (Have cmake) -and -not (Test-Path "$env:ProgramFiles\CMake\bin\cmake.exe")) {
    Step 'Installing CMake'
    winget install --id Kitware.CMake -e --accept-package-agreements --accept-source-agreements
    if ($LASTEXITCODE) { throw "CMake install failed ($LASTEXITCODE)" }
} else { Step 'CMake already installed, skipping' }

# 4. vcpkg cloned and bootstrapped; VCPKG_ROOT persisted so new shells see it.
if (-not (Test-Path (Join-Path $VcpkgRoot 'vcpkg.exe'))) {
    Step "Installing vcpkg to $VcpkgRoot"
    if (-not (Test-Path $VcpkgRoot)) { git clone https://github.com/microsoft/vcpkg.git $VcpkgRoot }
    & (Join-Path $VcpkgRoot 'bootstrap-vcpkg.bat') -disableMetrics
    if ($LASTEXITCODE) { throw 'vcpkg bootstrap failed' }
} else { Step 'vcpkg already installed, skipping' }
[Environment]::SetEnvironmentVariable('VCPKG_ROOT', $VcpkgRoot, 'User')
$env:VCPKG_ROOT = $VcpkgRoot

# 5. libtorrent and its deps (boost, openssl) as static libs with the static CRT.
# Manifest mode: the repo's vcpkg.json pins libtorrent 2.0.x (plain
# `vcpkg install libtorrent:x64-windows-static` would get the latest port, 2.1.x).
# Install root is %VCPKG_ROOT%\installed because the vcpkg Rust crate only reads that
# tree; manifest mode makes the tree match vcpkg.json (removes anything not listed).
Step 'vcpkg install (manifest, x64-windows-static; skips what is already built)'
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
    & (Join-Path $VcpkgRoot 'vcpkg.exe') install --triplet x64-windows-static --x-install-root (Join-Path $VcpkgRoot 'installed')
    if ($LASTEXITCODE) { throw 'vcpkg install failed' }
} finally { Pop-Location }

Step 'Done. Open a new shell, then: cargo test -p nyx-engine'
