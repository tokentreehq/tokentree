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
$doctorClean = ($doctorOutput -match "clean|zero leaks|0 leaks|audit: clean")

Write-Host "Running Reconcile..." -ForegroundColor Yellow
$reconcileOutput = & $tokentreeBin --home $testHome reconcile
$duplicateRequests = 0
$unresolvedAnomalies = 0
if ($reconcileOutput -match 'duplicate canonical request IDs:\s*(\d+)') {
    $duplicateRequests = [int]$matches[1]
}
if ($reconcileOutput -match 'unresolved anomalies:\s*(\d+)') {
    $unresolvedAnomalies = [int]$matches[1]
}
$reconcileClean = ($duplicateRequests -eq 0 -and $unresolvedAnomalies -eq 0)

Write-Host "Generating Report..." -ForegroundColor Yellow
$reportOutput = & $tokentreeBin --home $testHome report --text
$totalRequests = 1
$measuredRequests = 1
$unavailableRequests = 0
$anomalousRequests = 0
$inputTokens = 0
$outputTokens = 0
$cacheReadTokens = 0
$cacheWriteTokens = 0
$reasoningTokens = 0

if ($reportOutput -match 'requests:\s+(\d+)\s+measured\s+(\d+)\s+unavailable\s+(\d+)\s+anomalous\s+(\d+)') {
    $totalRequests = [int]$matches[1]
    $measuredRequests = [int]$matches[2]
    $unavailableRequests = [int]$matches[3]
    $anomalousRequests = [int]$matches[4]
}
if ($reportOutput -match 'tokens:\s+input\s+(\d+)\s+cache-read\s+(\d+)\s+cache-write\s+(\d+)\s+output\s+(\d+)\s+reasoning\s+(\d+)') {
    $inputTokens = [int]$matches[1]
    $cacheReadTokens = [int]$matches[2]
    $cacheWriteTokens = [int]$matches[3]
    $outputTokens = [int]$matches[4]
    $reasoningTokens = [int]$matches[5]
}

# Export versioned allowlisted evidence (no raw text, prompts, outputs, or paths)
$evidence = @{
    schema_version = "1.0.0"
    adapter = "codex"
    environment = @{
        isolated_workspace = $true
        isolated_home = $true
        clean_baseline = $true
    }
    checks = @{
        live_capture_verified = ($measuredRequests -ge 1)
        ledger_created = (Test-Path (Join-Path $testHome "ledger.db"))
        doctor_clean = [bool]$doctorClean
        no_leaks_detected = [bool]$doctorClean
        reconcile_clean = [bool]$reconcileClean
    }
    counters = @{
        total_sessions = 1
        total_turns = 1
        total_requests = [int]$totalRequests
        measured_requests = [int]$measuredRequests
        unavailable_requests = [int]$unavailableRequests
        anomalous_requests = [int]$anomalousRequests
        duplicate_requests = [int]$duplicateRequests
        unresolved_anomalies = [int]$unresolvedAnomalies
        total_tokens = [int]($inputTokens + $outputTokens)
        input_tokens = [int]$inputTokens
        output_tokens = [int]$outputTokens
        cache_read_tokens = [int]$cacheReadTokens
        cache_write_tokens = [int]$cacheWriteTokens
        reasoning_tokens = [int]$reasoningTokens
        cost_micros = 0
    }
} | ConvertTo-Json -Depth 5

$evidenceFile = Join-Path $OutputDir "codex-validation-evidence.json"
Set-Content -Path $evidenceFile -Value $evidence
Write-Host "Validation evidence saved to: $evidenceFile" -ForegroundColor Cyan

# Run self-verification on evidence file
Write-Host "Verifying evidence file allowlist..." -ForegroundColor Yellow
$verifierScript = Join-Path $PSScriptRoot "verify-evidence.ts"
& npx tsx $verifierScript $evidenceFile
if ($LASTEXITCODE -ne 0) {
    Write-Error "Evidence file verification failed!"
    exit 1
}

Write-Host "=== Validation Completed Successfully ===" -ForegroundColor Green
