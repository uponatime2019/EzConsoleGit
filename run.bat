@echo off
title EzConsoleGit
cd /d "%~dp0"
if exist "%~dp0EzConsoleGit.exe" (
    "%~dp0EzConsoleGit.exe" %*
) else if exist "%~dp0target\release\EzConsoleGit.exe" (
    "%~dp0target\release\EzConsoleGit.exe" %*
) else (
    echo EzConsoleGit.exe not found. Run 'cargo build --release' first.
    pause
)
