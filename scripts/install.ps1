$ErrorActionPreference = "Stop"
Set-StrictMode -Version 2.0

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot = Split-Path -Parent $ScriptDir

. (Join-Path $ScriptDir "lib.ps1")

$Tag = ""
$Branch = ""
$Commit = ""
$SkipBuildTools = $false

for ($index = 0; $index -lt $args.Count; $index += 1) {
    switch ($args[$index]) {
        "--tag" {
            if ($index + 1 -ge $args.Count) { Fail "missing value for --tag" }
            $index += 1
            $Tag = $args[$index]
        }
        "-Tag" {
            if ($index + 1 -ge $args.Count) { Fail "missing value for -Tag" }
            $index += 1
            $Tag = $args[$index]
        }
        "--branch" {
            if ($index + 1 -ge $args.Count) { Fail "missing value for --branch" }
            $index += 1
            $Branch = $args[$index]
        }
        "-Branch" {
            if ($index + 1 -ge $args.Count) { Fail "missing value for -Branch" }
            $index += 1
            $Branch = $args[$index]
        }
        "--commit" {
            if ($index + 1 -ge $args.Count) { Fail "missing value for --commit" }
            $index += 1
            $Commit = $args[$index]
        }
        "-Commit" {
            if ($index + 1 -ge $args.Count) { Fail "missing value for -Commit" }
            $index += 1
            $Commit = $args[$index]
        }
        "--skip-build-tools" { $SkipBuildTools = $true }
        "-SkipBuildTools" { $SkipBuildTools = $true }
        default { Fail "unknown option for install.ps1: $($args[$index])" }
    }
}

try {
    Require-CleanRefSelection $Tag $Branch $Commit
    Load-CargoEnv

    if (Test-AcsGloballyInstalled) {
        Write-Info "$script:AcsName is already installed at $(Get-AcsGlobalBinaryPath)"
        Write-Info "installed build: $(Get-InstalledVersionLine)"
        Write-Info "use ``acs update`` to replace the global build"
        exit 0
    }

    Ensure-RustToolchain -SkipBuildTools:$SkipBuildTools

    $tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("acs-install-{0}" -f [Guid]::NewGuid().ToString("N"))
    Ensure-Dir $tempRoot

    try {
        $sourceDir = Join-Path $tempRoot "source"
        $buildBranch = "unknown"
        $buildCommit = "unknown"
        $buildDirty = "clean"
        $buildSourceKind = "unknown"
        $buildSourceRef = "unknown"

        if ([string]::IsNullOrWhiteSpace($Tag) -and [string]::IsNullOrWhiteSpace($Branch) -and [string]::IsNullOrWhiteSpace($Commit)) {
            $Tag = Get-LatestRemoteTag
            if ([string]::IsNullOrWhiteSpace($Tag)) {
                Fail "no remote tags were found at $(Get-AcsRepoUrl)"
            }
            Write-Info "installing $script:AcsName from latest remote tag $Tag"
        } elseif (-not [string]::IsNullOrWhiteSpace($Tag)) {
            Write-Info "installing $script:AcsName from remote tag $Tag"
        }

        if (-not [string]::IsNullOrWhiteSpace($Tag)) {
            if (Test-CommandAvailable "git") {
                Clone-RemoteTagSource $Tag $sourceDir
                $buildCommit = Get-GitCommitId $sourceDir
            } else {
                $sourceDir = Download-GitHubSourceArchive "tag" $Tag $tempRoot
            }
            $buildBranch = "detached"
            $buildSourceKind = "remote-tag"
            $buildSourceRef = $Tag
        } elseif (-not [string]::IsNullOrWhiteSpace($Branch)) {
            Write-Info "installing $script:AcsName from remote branch $Branch"
            if (Test-CommandAvailable "git") {
                Clone-RemoteBranchSource $Branch $sourceDir
                $buildCommit = Get-GitCommitId $sourceDir
            } else {
                $sourceDir = Download-GitHubSourceArchive "branch" $Branch $tempRoot
            }
            $buildBranch = $Branch
            $buildSourceKind = "remote-branch"
            $buildSourceRef = $Branch
        } elseif (-not [string]::IsNullOrWhiteSpace($Commit)) {
            Write-Info "installing $script:AcsName from remote commit $Commit"
            if (Test-CommandAvailable "git") {
                Clone-RemoteCommitSource $Commit $sourceDir
                $buildCommit = Get-GitCommitId $sourceDir
            } else {
                $sourceDir = Download-GitHubSourceArchive "commit" $Commit $tempRoot
                $buildCommit = $Commit
            }
            $buildBranch = "detached"
            $buildSourceKind = "remote-commit"
            $buildSourceRef = $Commit
        }

        Install-BinaryFromSource `
            $sourceDir `
            $false `
            $buildBranch `
            $buildCommit `
            $buildDirty `
            $buildSourceKind `
            $buildSourceRef

        Ensure-StandardGlobalLayout
        Print-GlobalInstallSummary
    } finally {
        Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
} catch {
    Write-Error $_.Exception.Message
    exit 1
}
