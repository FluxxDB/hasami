<#
Regenerates the third-party license files that the installer ships under licenses\:
  THIRD_PARTY_LICENSES_RUST.md  every Rust crate compiled into hasami.exe (cargo-about)
  THIRD_PARTY_LICENSES_NPM.md   every npm package bundled into the UI (a real Vite build)

Run it after changing Cargo.lock or package-lock.json, and commit the results.
Needs cargo-about once:  cargo install cargo-about --locked --features cli

Usage (from the repo root):
  powershell.exe -ExecutionPolicy Bypass -File scripts/licenses.ps1
#>
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot

if (-not (Get-Command cargo-about -ErrorAction SilentlyContinue)) {
    throw 'cargo-about is not installed. Run: cargo install cargo-about --locked --features cli'
}

Push-Location (Join-Path $Root 'src-tauri')
try {
    cargo about generate --locked --fail `
        -c (Join-Path $PSScriptRoot 'about.toml') `
        -o (Join-Path $Root 'THIRD_PARTY_LICENSES_RUST.md') `
        (Join-Path $PSScriptRoot 'about.hbs')
    if ($LASTEXITCODE -ne 0) { throw "cargo about failed ($LASTEXITCODE)" }
}
finally {
    Pop-Location
}
Write-Host "Wrote $(Join-Path $Root 'THIRD_PARTY_LICENSES_RUST.md')"

Push-Location $Root
try {
    node scripts/npm-licenses.mjs
    if ($LASTEXITCODE -ne 0) { throw "npm-licenses.mjs failed ($LASTEXITCODE)" }
}
finally {
    Pop-Location
}
