# Bumps the version, tests, tags, and pushes. The Release workflow on GitHub
# then builds the exe and wheel, creates the GitHub release, and publishes to PyPI.
#
#   .\scripts\release.ps1 0.2.0
param(
    [Parameter(Mandatory)]
    [ValidatePattern('^\d+\.\d+\.\d+$')]
    [string]$Version
)

$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot)

function Invoke-Checked {
    & $args[0] $args[1..($args.Count - 1)]
    if ($LASTEXITCODE -ne 0) { throw "$($args -join ' ') failed with exit code $LASTEXITCODE" }
}

$tag = "v$Version"
$branch = git branch --show-current
if ($branch -ne 'main') { throw "Release from main (currently on '$branch')." }
if (git status --porcelain) { throw 'The working tree has uncommitted changes.' }
if (git tag --list $tag) { throw "Tag $tag already exists." }

$remote = git config "branch.main.remote"
if (-not $remote) { throw "main has no upstream; push it once with 'git push -u <remote> main'." }

$manifest = Get-Content Cargo.toml -Raw
$pattern = '(?m)^version = "[^"]+"'
if ($manifest -notmatch $pattern) { throw 'Could not find the package version in Cargo.toml.' }
# Only the first match is the [package] version; dependency tables use inline versions.
$manifest = ([regex]$pattern).Replace($manifest, "version = `"$Version`"", 1)
Set-Content Cargo.toml $manifest -NoNewline

Invoke-Checked cargo check --quiet   # refreshes Cargo.lock
Invoke-Checked cargo test --quiet

Invoke-Checked git add Cargo.toml Cargo.lock
Invoke-Checked git commit -m "Release $tag"
Invoke-Checked git tag -a $tag -m "Release $tag"
Invoke-Checked git push $remote main $tag

Write-Host "Pushed $tag. Follow the build with: gh run watch"
