# publish-release.ps1 - publishes dist\nyx.exe as a GitHub release on the PUBLIC releases repo.
# The source repo is public too (MIT); the releases repo holds the README and the downloads.
#   1. reads the version from Cargo.toml (workspace.package.version)
#   2. copies dist\nyx.exe to dist\Nyx-<version>-windows-x64.exe (the name the in-app updater looks for)
#      and writes the matching .sha256 file
#   3. tags HEAD v<version> in the source repo and pushes the tag, then zips that tag as
#      dist\Nyx-<version>-source.zip (git archive, so no build output or untracked files)
#   4. creates release v<version> on walkingcorpses/nyx-releases with the exe, .sha256 and source zip
# Commit and push the source first: the tag marks exactly what you released.
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
$src = "dist\Nyx-$version-source.zip"
git archive --format=zip --prefix="nyx-$version/" -o $src $tag
if ($LASTEXITCODE) { throw 'git archive failed' }

gh release create $tag $asset "$asset.sha256" $src --repo $Repo --title "Nyx $version" --notes-file $NotesFile
if ($LASTEXITCODE) { throw 'gh release create failed' }
Write-Host "Published $tag to $Repo"
