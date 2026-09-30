#!/usr/bin/env pwsh
# SPDX-License-Identifier: Apache-2.0
# TokenTree Claude Code Friend Validation Runner (Windows / PowerShell)

param (
    [string]$OutputDir = "$PSScriptRoot/evidence-claude"
)

$ErrorActionPreference = "Stop"

Write-Host "=== TokenTree Claude Code Validation Runner ===" -ForegroundColor Cyan

# 1. Verify Claude CLI
if (-not (Get-Command claude -ErrorAction SilentlyContinue)) {
    Write-Error "Claude CLI ('claude') not found in PATH. Please install Claude Code before running this script."
    exit 1
}

$claudeVersion = claude --version
Write-Host "Detected Claude CLI: $claudeVersion" -ForegroundColor Green

# 2. Setup isolated test environment
$testHome = Join-Path $OutputDir "tokentree-home"
$testWorkspace = Join-Path $OutputDir "test-workspace"
if (Test-Path $OutputDir) { Remove-Item -Recurse -Force $OutputDir }
New-Item -ItemType Directory -Path $testHome -Force | Out-Null
New-Item -ItemType Directory -Path $testWorkspace -Force | Out-Null

$packageJson = @{
    name = "tokentree-live-validation"
    version = "1.0.0"
    private = $true
} | ConvertTo-Json
Set-Content -Path (Join-Path $testWorkspace "package.json") -Value $packageJson

Write-Host "Created isolated workspace at $testWorkspace"

# 3. Build TokenTree
$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "../..")
Push-Location $repoRoot
try {
    Write-Host "Compiling TokenTree CLI..." -ForegroundColor Yellow
    cargo build -p tokentree-cli
    $tokentreeBin = Join-Path $repoRoot "target/debug/tokentree.exe"
} finally {
    Pop-Location
}

# 4. Execute test prompt with Claude
Push-Location $testWorkspace
try {
    Write-Host "Executing live Claude test prompt..." -ForegroundColor Yellow
    $prompt = "Reply with exactly: TokenTree live capture verified."
    $output = claude -p $prompt
    Write-Host "Claude response received: $output" -ForegroundColor Green
} finally {
    Pop-Location
}

# 5. Ingest and reconcile
Write-Host "Processing capture..." -ForegroundColor Yellow
& $tokentreeBin --home $testHome classify

Write-Host "Running Doctor..." -ForegroundColor Yellow
$doctorOutput = & $tokentreeBin --home $testHome doctor

Write-Host "Running Reconcile..." -ForegroundColor Yellow
$reconcileOutput = & $tokentreeBin --home $testHome reconcile

Write-Host "Generating Report..." -ForegroundColor Yellow
$reportOutput = & $tokentreeBin --home $testHome report --text

# 6. Export redacted evidence
$evidence = @{
    timestamp = (Get-Date -Format o)
    host = @{
        os = [System.Environment]::OSVersion.VersionString
        platform = "windows"
        claude_version = "$claudeVersion"
    }
    doctor = $doctorOutput
    reconcile = $reconcileOutput
    report = $reportOutput
} | ConvertTo-Json -Depth 5

$evidenceFile = Join-Path $OutputDir "claude-validation-evidence.json"
Set-Content -Path $evidenceFile -Value $evidence
Write-Host "Validation evidence saved to: $evidenceFile" -ForegroundColor Cyan
Write-Host "=== Validation Completed Successfully ===" -ForegroundColor Green
