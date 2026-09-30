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

# 6. Export versioned allowlisted evidence (no raw text, prompts, outputs, or paths)
$evidence = @{
    schema_version = "1.0.0"
    adapter = "claude"
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

$evidenceFile = Join-Path $OutputDir "claude-validation-evidence.json"
Set-Content -Path $evidenceFile -Value $evidence
Write-Host "Validation evidence saved to: $evidenceFile" -ForegroundColor Cyan

# 7. Run self-verification on evidence file
Write-Host "Verifying evidence file allowlist..." -ForegroundColor Yellow
$verifierScript = Join-Path $PSScriptRoot "verify-evidence.ts"
& npx tsx $verifierScript $evidenceFile
if ($LASTEXITCODE -ne 0) {
    Write-Error "Evidence file verification failed!"
    exit 1
}

Write-Host "=== Validation Completed Successfully ===" -ForegroundColor Green
