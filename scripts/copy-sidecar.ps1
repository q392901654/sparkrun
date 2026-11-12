# Copy sparkrun-sidecar into extensions/vscode/bin for VSIX packaging.
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$exe = if ($env:OS -match "Windows") { "sparkrun-sidecar.exe" } else { "sparkrun-sidecar" }
$destDir = Join-Path $root "extensions\vscode\bin"
New-Item -ItemType Directory -Force -Path $destDir | Out-Null

$candidates = @(
    (Join-Path $root "target\release\$exe"),
    (Join-Path $root "target\debug\$exe")
)
$src = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $src) {
    Write-Error "sidecar binary not found. Run: cargo build -p sparkrun-sidecar --release"
}
Copy-Item -Force $src (Join-Path $destDir $exe)
Write-Host "Copied $src -> $destDir\$exe"
