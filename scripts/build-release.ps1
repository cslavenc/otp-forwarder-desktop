# Builds the Windows installer and the portable exe in one go and copies both to .\release\.
# Usage: npm run release   (or: powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)

function Step($name, [scriptblock]$run) {
    Write-Host "`n==> $name" -ForegroundColor Cyan
    & $run
    if ($LASTEXITCODE -ne 0) { throw "$name failed (exit code $LASTEXITCODE)" }
}

foreach ($tool in 'node', 'npm', 'cargo') {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
        throw "$tool not found. See the Development prerequisites in README.md."
    }
}

$cargo = 'src-tauri\Cargo.toml'
Step 'Install JS dependencies' { npm ci }
Step 'Frontend type-check' { npm run check }
Step 'Rust tests' { cargo test --manifest-path $cargo }
Step 'Rust lint' { cargo clippy --manifest-path $cargo --all-targets -- -D warnings }
Step 'Build installer' { npm run tauri build }

$version = (Get-Content src-tauri\tauri.conf.json -Raw | ConvertFrom-Json).version
$out = 'release'
New-Item -ItemType Directory -Force $out | Out-Null
Remove-Item "$out\*" -Force
Copy-Item "src-tauri\target\release\bundle\nsis\OTP Forwarder_${version}_x64-setup.exe" $out
Copy-Item 'src-tauri\target\release\otp-forwarder-desktop.exe' "$out\OTP Forwarder_${version}_x64-portable.exe"

Write-Host "`nDone. Files in $(Resolve-Path $out):" -ForegroundColor Green
Get-ChildItem $out | ForEach-Object { '  {0}  ({1:N1} MB)' -f $_.Name, ($_.Length / 1MB) }
