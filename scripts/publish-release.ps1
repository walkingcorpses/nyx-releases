# publish-release.ps1 - publishes dist\nyx.exe as a GitHub release of this repo (walkingcorpses/nyx-releases, public, MIT).
#   1. reads the version from Cargo.toml (workspace.package.version)
#   2. copies dist\nyx.exe to dist\Nyx-<version>-windows-x64.exe (the name the in-app updater looks for)
#      and writes the matching .sha256 file
#   3. tags HEAD v<version> and pushes it (origin is this repo), so the release's "Source code" archive
#      is exactly what was built
#   4. creates release v<version> with the exe and the .sha256 file
# Commit and push first: the tag marks exactly what you released.
# Build first (scripts\build-release.ps1) and look at the exe before running this: it makes the
# release public. Usage: powershell -File scripts\publish-release.ps1 -NotesFile notes.md

param(
    [Parameter(Mandatory)][string]$NotesFile,
    [string]$Repo = 'walkingcorpses/nyx-releases'
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$tag = "v$version"
if (-not (Test-Path dist\nyx.exe)) { throw 'dist\nyx.exe is missing; run scripts\build-release.ps1 first' }
# gh prints "release not found" on stderr when the tag is free; that must not stop the script.
$ErrorActionPreference = 'Continue'
gh release view $tag --repo $Repo *> $null
$exists = ($LASTEXITCODE -eq 0)
$ErrorActionPreference = 'Stop'
if ($exists) { throw "release $tag already exists on $Repo" }

$asset = "dist\Nyx-$version-windows-x64.exe"
Copy-Item dist\nyx.exe $asset -Force
$hash = (Get-FileHash $asset -Algorithm SHA256).Hash.ToLower()
"$hash  $(Split-Path $asset -Leaf)" | Set-Content -Encoding ascii "$asset.sha256"
Write-Host "SHA-256: $hash"

# A dirty tree means the tag would not match what was built.
if (git status --porcelain) { throw 'working tree has uncommitted changes; commit them first' }
git tag $tag
if ($LASTEXITCODE) { throw "git tag $tag failed (already exists?)" }
git push origin $tag
if ($LASTEXITCODE) { throw 'git push of the tag failed' }

gh release create $tag $asset "$asset.sha256" --repo $Repo --title "Nyx $version" --notes-file $NotesFile
if ($LASTEXITCODE) { throw 'gh release create failed' }
Write-Host "Published $tag to $Repo"
