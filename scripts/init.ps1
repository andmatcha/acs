$ErrorActionPreference = "Stop"
Set-StrictMode -Version 2.0

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot = Split-Path -Parent $ScriptDir

. (Join-Path $ScriptDir "lib.ps1")

$SkipBuildTools = $false

for ($index = 0; $index -lt $args.Count; $index += 1) {
    switch ($args[$index]) {
        "--skip-build-tools" { $SkipBuildTools = $true }
        "-SkipBuildTools" { $SkipBuildTools = $true }
        default { Fail "unknown option for init.ps1: $($args[$index])" }
    }
}

try {
    Ensure-RustToolchain -WithComponents -SkipBuildTools:$SkipBuildTools

    Push-Location $RepoRoot
    try {
        Invoke-NativeCommand "rustup" @("override", "set", "stable") "failed to set the local Rust override"
    } finally {
        Pop-Location
    }

    Write-Info "building a local release binary"
    Invoke-NativeCommand "cargo" @("build", "--manifest-path", (Join-Path $RepoRoot "Cargo.toml"), "--release") "cargo build failed"

    Write-Info "local launcher is ready at $(Join-Path $RepoRoot 'acs.cmd')"
    Write-Info "run it from this directory with: .\acs.cmd --help"
} catch {
    Write-Error $_.Exception.Message
    exit 1
}
