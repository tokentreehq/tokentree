#!/usr/bin/env pwsh
# SPDX-License-Identifier: Apache-2.0
# TokenTree Codex Friend Validation Runner (Windows / PowerShell)

param (
    [string]$CodexSessionsDir = "$HOME/.codex/sessions",
    [string]$OutputDir = "$PSScriptRoot/evidence-codex"
)

$ErrorActionPreference = "Stop"

Write-Host "=== TokenTree OpenAI Codex Validation Runner ===" -ForegroundColor Cyan

if (-not (Test-Path $CodexSessionsDir)) {
    Write-Host "Warning: Codex sessions directory not found at $CodexSessionsDir" -ForegroundColor Yellow
    Write-Host "Please provide the path using -CodexSessionsDir <path>"
}

$testHome = Join-Path $OutputDir "tokentree-home"
if (Test-Path $OutputDir) { Remove-Item -Recurse -Force $OutputDir }
New-Item -ItemType Directory -Path $testHome -Force | Out-Null

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "../..")
Push-Location $repoRoot
try {
    Write-Host "Compiling TokenTree CLI..." -ForegroundColor Yellow
    cargo build -p tokentree-cli
    $tokentreeBin = Join-Path $repoRoot "target/debug/tokentree.exe"
} finally {
    Pop-Location
}

if (Test-Path $CodexSessionsDir) {
    Write-Host "Importing Codex sessions from $CodexSessionsDir..." -ForegroundColor Yellow
    $importOutput = & $tokentreeBin --home $testHome import codex $CodexSessionsDir
    Write-Host "Import result: $importOutput" -ForegroundColor Green
}

Write-Host "Running Doctor..." -ForegroundColor Yellow
$doctorOutput = & $tokentreeBin --home $testHome doctor

Write-Host "Running Reconcile..." -ForegroundColor Yellow
$reconcileOutput = & $tokentreeBin --home $testHome reconcile

Write-Host "Generating Report..." -ForegroundColor Yellow
$reportOutput = & $tokentreeBin --home $testHome report --text

$evidence = @{
    timestamp = (Get-Date -Format o)
    host = @{
        os = [System.Environment]::OSVersion.VersionString
        platform = "windows"
        codex_path = $CodexSessionsDir
    }
    doctor = $doctorOutput
    reconcile = $reconcileOutput
    report = $reportOutput
} | ConvertTo-Json -Depth 5

$evidenceFile = Join-Path $OutputDir "codex-validation-evidence.json"
Set-Content -Path $evidenceFile -Value $evidence
Write-Host "Validation evidence saved to: $evidenceFile" -ForegroundColor Cyan
Write-Host "=== Validation Completed Successfully ===" -ForegroundColor Green
