<#
.SYNOPSIS
    Fetches and verifies the pinned Chrome for Testing build declared in
    crates/florui-conformance/chromium-pin.json, extracting it under
    .tools/chromium/<version>/ so `florui compare` finds it automatically
    with no --chromium flag needed.

.DESCRIPTION
    Idempotent: does nothing if the pinned version is already present.
    Verifies the downloaded archive's SHA256 against the pin before
    extracting, so a corrupted download or a tampered mirror is rejected
    rather than silently used.
#>
$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$pinPath = Join-Path $repoRoot "crates/florui-conformance/chromium-pin.json"
$pin = Get-Content -Raw -Path $pinPath | ConvertFrom-Json

$destRoot = Join-Path $repoRoot ".tools/chromium/$($pin.version)"
$exePath = Join-Path $destRoot "chrome-win64/chrome.exe"

if (Test-Path $exePath) {
    Write-Host "Chrome for Testing $($pin.version) already present at $exePath"
    exit 0
}

New-Item -ItemType Directory -Force -Path $destRoot | Out-Null
$zipPath = Join-Path $destRoot "chrome-win64.zip"

Write-Host "Downloading Chrome for Testing $($pin.version) ($($pin.platform)) from $($pin.url) ..."
Invoke-WebRequest -Uri $pin.url -OutFile $zipPath

$actualHash = (Get-FileHash -Path $zipPath -Algorithm SHA256).Hash.ToLowerInvariant()
$expectedHash = $pin.sha256.ToLowerInvariant()
if ($actualHash -ne $expectedHash) {
    Remove-Item -Force $zipPath
    throw "SHA256 mismatch for Chrome for Testing $($pin.version): expected $expectedHash, got $actualHash. Refusing to extract an unverified download."
}
Write-Host "Checksum verified ($actualHash)."

Write-Host "Extracting to $destRoot ..."
Expand-Archive -Path $zipPath -DestinationPath $destRoot -Force
Remove-Item -Force $zipPath

if (-not (Test-Path $exePath)) {
    throw "Extraction succeeded but $exePath was not produced - the archive layout may have changed."
}

Write-Host "Chrome for Testing $($pin.version) ready at $exePath"
