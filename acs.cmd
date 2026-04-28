@echo off
setlocal
set "REPO_ROOT=%~dp0"

if defined CARGO_HOME (
    set "CARGO_BIN=%CARGO_HOME%\bin"
) else (
    set "CARGO_BIN=%USERPROFILE%\.cargo\bin"
)

set "PATH=%CARGO_BIN%;%PATH%"

where cargo.exe >nul 2>nul
if errorlevel 1 (
    echo cargo was not found. Run scripts\init.cmd first. 1>&2
    exit /b 1
)

cargo run --manifest-path "%REPO_ROOT%Cargo.toml" --release -- %*
exit /b %ERRORLEVEL%
