$ErrorActionPreference = "Stop"
Set-StrictMode -Version 2.0

$script:AcsName = "acs"
$script:AcsRepoUrlDefault = "https://github.com/andmatcha/acs.git"

function Write-Info {
    param([string]$Message)
    Write-Host $Message
}

function Fail {
    param([string]$Message)
    throw $Message
}

function Test-WindowsHost {
    if ($env:OS -eq "Windows_NT") {
        return $true
    }
    if ($PSVersionTable.ContainsKey("Platform") -and $PSVersionTable.Platform -eq "Win32NT") {
        return $true
    }
    return $false
}

function Get-AcsRepoUrl {
    if (-not [string]::IsNullOrWhiteSpace($env:ACS_REPO_URL)) {
        return $env:ACS_REPO_URL
    }
    return $script:AcsRepoUrlDefault
}

function Get-AcsHomeDir {
    if (-not [string]::IsNullOrWhiteSpace($env:USERPROFILE)) {
        return $env:USERPROFILE
    }
    if (-not [string]::IsNullOrWhiteSpace($env:HOME)) {
        return $env:HOME
    }
    Fail "neither USERPROFILE nor HOME is set"
}

function Get-AcsLogDir {
    if (-not [string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
        $baseDir = $env:LOCALAPPDATA
    } else {
        $baseDir = Join-Path (Get-AcsHomeDir) "AppData\Local"
    }
    return Join-Path (Join-Path $baseDir $script:AcsName) "logs"
}

function Get-AcsCargoHome {
    if (-not [string]::IsNullOrWhiteSpace($env:CARGO_HOME)) {
        return $env:CARGO_HOME
    }
    return Join-Path (Get-AcsHomeDir) ".cargo"
}

function Get-AcsCargoBinDir {
    return Join-Path (Get-AcsCargoHome) "bin"
}

function Get-AcsGlobalBinaryPath {
    if (Test-WindowsHost) {
        return Join-Path (Get-AcsCargoBinDir) "$script:AcsName.exe"
    }
    return Join-Path (Get-AcsCargoBinDir) $script:AcsName
}

function Add-PathEntry {
    param([string]$PathEntry)

    if ([string]::IsNullOrWhiteSpace($PathEntry)) {
        return
    }

    $separator = [System.IO.Path]::PathSeparator
    $currentEntries = @()
    if (-not [string]::IsNullOrWhiteSpace($env:PATH)) {
        $currentEntries = $env:PATH -split [regex]::Escape([string]$separator)
    }

    foreach ($entry in $currentEntries) {
        if ((Normalize-ComparablePath $entry).Equals((Normalize-ComparablePath $PathEntry), [System.StringComparison]::OrdinalIgnoreCase)) {
            return
        }
    }

    if ([string]::IsNullOrWhiteSpace($env:PATH)) {
        $env:PATH = $PathEntry
    } else {
        $env:PATH = "$PathEntry$separator$env:PATH"
    }
}

function Load-CargoEnv {
    Add-PathEntry (Get-AcsCargoBinDir)
}

function Normalize-ComparablePath {
    param([string]$PathValue)
    return $PathValue.TrimEnd(@([char]'\', [char]'/'))
}

function Test-CommandAvailable {
    param([string]$Name)
    return [bool](Get-Command $Name -ErrorAction SilentlyContinue)
}

function Require-Command {
    param([string]$Name)
    if (-not (Test-CommandAvailable $Name)) {
        Fail "required command not found: $Name"
    }
}

function Invoke-NativeCommand {
    param(
        [string]$File,
        [string[]]$Arguments = @(),
        [string]$ErrorMessage = ""
    )

    & $File @Arguments
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        if ([string]::IsNullOrWhiteSpace($ErrorMessage)) {
            $ErrorMessage = "command failed: $File $($Arguments -join ' ')"
        }
        Fail "$ErrorMessage (exit code $exitCode)"
    }
}

function Invoke-NativeOutput {
    param(
        [string]$File,
        [string[]]$Arguments = @()
    )

    $output = & $File @Arguments 2>$null
    if ($LASTEXITCODE -ne 0) {
        return $null
    }

    $text = ($output -join "`n").Trim()
    if ($text.Length -eq 0) {
        return $null
    }
    return $text
}

function Require-CleanRefSelection {
    param(
        [string]$Tag,
        [string]$Branch,
        [string]$Commit
    )

    $count = 0
    if (-not [string]::IsNullOrWhiteSpace($Tag)) { $count += 1 }
    if (-not [string]::IsNullOrWhiteSpace($Branch)) { $count += 1 }
    if (-not [string]::IsNullOrWhiteSpace($Commit)) { $count += 1 }
    if ($count -gt 1) {
        Fail "specify only one of Tag, Branch, or Commit"
    }
}

function Ensure-Dir {
    param([string]$Path)
    New-Item -ItemType Directory -Force -Path $Path | Out-Null
}

function Test-AcsGloballyInstalled {
    return (Test-Path -LiteralPath (Get-AcsGlobalBinaryPath) -PathType Leaf)
}

function Get-InstalledVersionLine {
    if (-not (Test-AcsGloballyInstalled)) {
        return $null
    }

    $binaryPath = Get-AcsGlobalBinaryPath
    $output = & $binaryPath --version 2>$null
    if ($LASTEXITCODE -ne 0 -or $null -eq $output) {
        return "version metadata unavailable"
    }

    $text = ($output -join "`n").Trim()
    if ($text.Length -eq 0) {
        return "version metadata unavailable"
    }
    return $text
}

function Test-GitRepository {
    param([string]$RepoDir)
    if (-not (Test-CommandAvailable "git")) {
        return $false
    }

    & git -C $RepoDir rev-parse --is-inside-work-tree *> $null
    return ($LASTEXITCODE -eq 0)
}

function Get-GitBranchName {
    param([string]$RepoDir)

    $branch = Invoke-NativeOutput "git" @("-C", $RepoDir, "rev-parse", "--abbrev-ref", "HEAD")
    if ([string]::IsNullOrWhiteSpace($branch)) {
        return "unknown"
    }
    if ($branch -eq "HEAD") {
        return "detached"
    }
    return $branch
}

function Get-GitCommitId {
    param([string]$RepoDir)

    $commit = Invoke-NativeOutput "git" @("-C", $RepoDir, "rev-parse", "HEAD")
    if ([string]::IsNullOrWhiteSpace($commit)) {
        return "unknown"
    }
    return $commit
}

function Get-GitDirtyState {
    param([string]$RepoDir)

    $status = Invoke-NativeOutput "git" @("-C", $RepoDir, "status", "--porcelain")
    if ([string]::IsNullOrWhiteSpace($status)) {
        return "clean"
    }
    return "dirty"
}

function Get-SourceRefForBranch {
    param(
        [string]$Branch,
        [string]$Commit
    )

    if ($Branch -eq "detached" -or [string]::IsNullOrWhiteSpace($Branch) -or $Branch -eq "unknown") {
        return $Commit
    }
    return $Branch
}

function Clone-LocalHeadSource {
    param(
        [string]$RepoDir,
        [string]$DestinationDir
    )

    $commit = Get-GitCommitId $RepoDir
    Invoke-NativeCommand "git" @("clone", "--local", "--no-hardlinks", $RepoDir, $DestinationDir) "failed to clone local source"
    Invoke-NativeCommand "git" @("-c", "advice.detachedHead=false", "-C", $DestinationDir, "checkout", "--detach", $commit) "failed to check out local source"
}

function Clone-RemoteBranchSource {
    param(
        [string]$Branch,
        [string]$DestinationDir
    )
    Invoke-NativeCommand "git" @("clone", "--depth", "1", "--branch", $Branch, (Get-AcsRepoUrl), $DestinationDir) "failed to clone remote branch $Branch"
}

function Clone-RemoteTagSource {
    param(
        [string]$Tag,
        [string]$DestinationDir
    )
    Invoke-NativeCommand "git" @("-c", "advice.detachedHead=false", "clone", "--depth", "1", "--branch", $Tag, (Get-AcsRepoUrl), $DestinationDir) "failed to clone remote tag $Tag"
}

function Clone-RemoteCommitSource {
    param(
        [string]$Commit,
        [string]$DestinationDir
    )
    Invoke-NativeCommand "git" @("clone", (Get-AcsRepoUrl), $DestinationDir) "failed to clone remote repository"
    Invoke-NativeCommand "git" @("-c", "advice.detachedHead=false", "-C", $DestinationDir, "checkout", "--detach", $Commit) "failed to check out remote commit $Commit"
}

function Enable-Tls12 {
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    } catch {
        # Older hosts may not expose ServicePointManager in the same way. Invoke-WebRequest will report the real error.
    }
}

function Download-File {
    param(
        [string]$Uri,
        [string]$DestinationPath
    )

    Enable-Tls12
    $parameters = @{
        Uri = $Uri
        OutFile = $DestinationPath
    }
    if ($PSVersionTable.PSVersion.Major -lt 6) {
        $parameters.UseBasicParsing = $true
    }

    try {
        Invoke-WebRequest @parameters
    } catch {
        Fail "failed to download $Uri`: $($_.Exception.Message)"
    }
}

function Get-GitHubSlugFromRepoUrl {
    $repoUrl = Get-AcsRepoUrl
    $patterns = @(
        "^https://github\.com/(?<owner>[^/]+)/(?<repo>[^/]+?)(?:\.git)?/?$",
        "^git@github\.com:(?<owner>[^/]+)/(?<repo>[^/]+?)(?:\.git)?$",
        "^ssh://git@github\.com/(?<owner>[^/]+)/(?<repo>[^/]+?)(?:\.git)?/?$"
    )

    foreach ($pattern in $patterns) {
        $match = [regex]::Match($repoUrl, $pattern, [System.Text.RegularExpressions.RegexOptions]::IgnoreCase)
        if ($match.Success) {
            return "$($match.Groups["owner"].Value)/$($match.Groups["repo"].Value)"
        }
    }

    return $null
}

function Join-GitHubRefPath {
    param([string]$Ref)
    return (($Ref -split "/") | ForEach-Object { [Uri]::EscapeDataString($_) }) -join "/"
}

function Get-GitHubArchiveUrl {
    param(
        [string]$Kind,
        [string]$Ref
    )

    $slug = Get-GitHubSlugFromRepoUrl
    if ([string]::IsNullOrWhiteSpace($slug)) {
        Fail "Git is required for ACS_REPO_URL=$(Get-AcsRepoUrl); without Git, only GitHub repository URLs can be downloaded as archives"
    }

    $encodedRef = Join-GitHubRefPath $Ref
    switch ($Kind) {
        "tag" { return "https://github.com/$slug/archive/refs/tags/$encodedRef.zip" }
        "branch" { return "https://github.com/$slug/archive/refs/heads/$encodedRef.zip" }
        "commit" { return "https://github.com/$slug/archive/$encodedRef.zip" }
        default { Fail "unknown GitHub archive kind: $Kind" }
    }
}

function Find-ExtractedSourceRoot {
    param([string]$ExtractRoot)

    $direct = @(Get-ChildItem -LiteralPath $ExtractRoot -Force -Directory | Where-Object {
        Test-Path -LiteralPath (Join-Path $_.FullName "Cargo.toml") -PathType Leaf
    })
    if ($direct.Count -gt 0) {
        return $direct[0].FullName
    }

    $cargoToml = Get-ChildItem -LiteralPath $ExtractRoot -Force -Recurse -Filter "Cargo.toml" -File | Select-Object -First 1
    if ($null -ne $cargoToml) {
        return $cargoToml.DirectoryName
    }

    Fail "downloaded archive does not contain Cargo.toml"
}

function Download-GitHubSourceArchive {
    param(
        [string]$Kind,
        [string]$Ref,
        [string]$TempRoot
    )

    $archiveUrl = Get-GitHubArchiveUrl $Kind $Ref
    $zipPath = Join-Path $TempRoot "source.zip"
    $extractRoot = Join-Path $TempRoot "archive"

    Write-Info "Git was not found; downloading $archiveUrl"
    Download-File $archiveUrl $zipPath
    Ensure-Dir $extractRoot
    Expand-Archive -LiteralPath $zipPath -DestinationPath $extractRoot -Force
    return (Find-ExtractedSourceRoot $extractRoot)
}

function Copy-SourceTree {
    param(
        [string]$SourceDir,
        [string]$DestinationDir
    )

    Ensure-Dir $DestinationDir
    $excludedNames = @(".git", "target", "logs")
    Get-ChildItem -LiteralPath $SourceDir -Force | Where-Object {
        $excludedNames -notcontains $_.Name
    } | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination $DestinationDir -Recurse -Force
    }
}

function Get-WindowsRustupHostTriple {
    $architecture = $env:PROCESSOR_ARCHITEW6432
    if ([string]::IsNullOrWhiteSpace($architecture)) {
        $architecture = $env:PROCESSOR_ARCHITECTURE
    }
    if ($architecture -eq "ARM64") {
        return "aarch64-pc-windows-msvc"
    }
    if ($architecture -eq "x86") {
        return "i686-pc-windows-msvc"
    }
    return "x86_64-pc-windows-msvc"
}

function Install-RustupIfNeeded {
    Load-CargoEnv
    if (Test-CommandAvailable "rustup") {
        return
    }
    if (-not (Test-WindowsHost)) {
        Fail "rustup was not found; use scripts/init.sh on non-Windows hosts"
    }

    $tempPath = Join-Path ([System.IO.Path]::GetTempPath()) ("rustup-init-{0}.exe" -f [Guid]::NewGuid().ToString("N"))
    try {
        $hostTriple = Get-WindowsRustupHostTriple
        Write-Info "installing rustup with the stable toolchain"
        Download-File "https://static.rust-lang.org/rustup/dist/$hostTriple/rustup-init.exe" $tempPath
        Invoke-NativeCommand $tempPath @("-y", "--profile", "minimal", "--default-toolchain", "stable") "rustup installer failed"
    } finally {
        Remove-Item -LiteralPath $tempPath -Force -ErrorAction SilentlyContinue
    }

    Load-CargoEnv
}

function Get-StableRustHost {
    $output = Invoke-NativeOutput "rustup" @("run", "stable", "rustc", "-vV")
    if ([string]::IsNullOrWhiteSpace($output)) {
        return $null
    }

    foreach ($line in ($output -split "`n")) {
        if ($line -match "^host:\s*(.+)$") {
            return $matches[1].Trim()
        }
    }
    return $null
}

function Test-VcBuildToolsInstalled {
    if (Test-CommandAvailable "cl.exe") {
        return $true
    }
    if (Test-CommandAvailable "link.exe") {
        return $true
    }

    $programFilesX86 = [Environment]::GetEnvironmentVariable("ProgramFiles(x86)")
    if ([string]::IsNullOrWhiteSpace($programFilesX86)) {
        return $false
    }

    $vswhere = Join-Path $programFilesX86 "Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path -LiteralPath $vswhere -PathType Leaf)) {
        return $false
    }

    $installationPath = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null
    if ($LASTEXITCODE -ne 0) {
        return $false
    }
    return (-not [string]::IsNullOrWhiteSpace(($installationPath -join "`n").Trim()))
}

function Ensure-WindowsBuildTools {
    param([switch]$SkipInstall)

    if (-not (Test-WindowsHost)) {
        return
    }
    if (Test-VcBuildToolsInstalled) {
        return
    }
    if ($SkipInstall) {
        Write-Info "Microsoft C++ Build Tools were not found; cargo may fail to link Windows binaries"
        return
    }

    $winget = Get-Command "winget" -ErrorAction SilentlyContinue
    if ($null -ne $winget) {
        Write-Info "installing Microsoft C++ Build Tools with winget"
        Invoke-NativeCommand $winget.Source @(
            "install",
            "--id", "Microsoft.VisualStudio.2022.BuildTools",
            "--exact",
            "--source", "winget",
            "--silent",
            "--accept-package-agreements",
            "--accept-source-agreements",
            "--override", "--wait --quiet --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
        ) "failed to install Microsoft C++ Build Tools"
    } else {
        $bootstrapperPath = Join-Path ([System.IO.Path]::GetTempPath()) ("vs-buildtools-{0}.exe" -f [Guid]::NewGuid().ToString("N"))
        try {
            Write-Info "winget was not found; downloading the Visual Studio Build Tools bootstrapper"
            Download-File "https://aka.ms/vs/17/release/vs_BuildTools.exe" $bootstrapperPath
            Invoke-NativeCommand $bootstrapperPath @(
                "--wait",
                "--quiet",
                "--norestart",
                "--add", "Microsoft.VisualStudio.Workload.VCTools",
                "--includeRecommended"
            ) "failed to install Microsoft C++ Build Tools"
        } finally {
            Remove-Item -LiteralPath $bootstrapperPath -Force -ErrorAction SilentlyContinue
        }
    }

    if (-not (Test-VcBuildToolsInstalled)) {
        Write-Info "Build Tools installation finished, but this shell cannot see them yet. If cargo cannot link, restart the terminal and rerun the script."
    }
}

function Ensure-RustToolchain {
    param(
        [switch]$WithComponents,
        [switch]$SkipBuildTools
    )

    if ((Test-WindowsHost) -and -not (Test-CommandAvailable "rustup")) {
        Ensure-WindowsBuildTools -SkipInstall:$SkipBuildTools
    }

    Install-RustupIfNeeded
    Load-CargoEnv
    Require-Command "rustup"
    Require-Command "cargo"

    Write-Info "ensuring the stable Rust toolchain is available"
    Invoke-NativeCommand "rustup" @("toolchain", "install", "stable", "--profile", "minimal") "failed to install the stable Rust toolchain"

    $stableHost = Get-StableRustHost
    if (-not [string]::IsNullOrWhiteSpace($stableHost) -and $stableHost -like "*windows-msvc") {
        Ensure-WindowsBuildTools -SkipInstall:$SkipBuildTools
    }

    if ($WithComponents) {
        Invoke-NativeCommand "rustup" @("component", "add", "clippy", "rustfmt") "failed to install Rust components"
    }
}

function Install-BinaryFromSource {
    param(
        [string]$SourceDir,
        [bool]$ForceInstall,
        [string]$BuildBranch,
        [string]$BuildCommit,
        [string]$BuildDirty,
        [string]$BuildSourceKind,
        [string]$BuildSourceRef
    )

    $metadata = @{
        ACS_BUILD_BRANCH = $BuildBranch
        ACS_BUILD_COMMIT = $BuildCommit
        ACS_BUILD_DIRTY = $BuildDirty
        ACS_BUILD_SOURCE_KIND = $BuildSourceKind
        ACS_BUILD_SOURCE_REF = $BuildSourceRef
    }
    $previous = @{}

    foreach ($key in $metadata.Keys) {
        $previous[$key] = [Environment]::GetEnvironmentVariable($key, "Process")
        Set-Item -Path "Env:$key" -Value $metadata[$key]
    }

    try {
        $arguments = @("install", "--path", $SourceDir, "--locked")
        if ($ForceInstall) {
            $arguments += "--force"
        }
        Invoke-NativeCommand "cargo" $arguments "cargo install failed"
    } finally {
        foreach ($key in $metadata.Keys) {
            if ($null -eq $previous[$key]) {
                Remove-Item -Path "Env:$key" -ErrorAction SilentlyContinue
            } else {
                Set-Item -Path "Env:$key" -Value $previous[$key]
            }
        }
    }
}

function Ensure-StandardGlobalLayout {
    Ensure-Dir (Get-AcsLogDir)
}

function Test-PathContainsCargoBin {
    $cargoBinDir = Get-AcsCargoBinDir
    if ([string]::IsNullOrWhiteSpace($env:PATH)) {
        return $false
    }

    $separator = [System.IO.Path]::PathSeparator
    foreach ($entry in ($env:PATH -split [regex]::Escape([string]$separator))) {
        if ((Normalize-ComparablePath $entry).Equals((Normalize-ComparablePath $cargoBinDir), [System.StringComparison]::OrdinalIgnoreCase)) {
            return $true
        }
    }
    return $false
}

function Print-PathHintIfNeeded {
    if (-not (Test-PathContainsCargoBin)) {
        Write-Info "add $(Get-AcsCargoBinDir) to PATH if you want to run ``$script:AcsName`` without a full path"
    }
}

function Print-GlobalInstallSummary {
    Write-Info "binary installed to $(Get-AcsGlobalBinaryPath)"
    Write-Info "log dir: $(Get-AcsLogDir)"
    if (Test-AcsGloballyInstalled) {
        Write-Info "installed build: $(Get-InstalledVersionLine)"
    }
    Print-PathHintIfNeeded
}
