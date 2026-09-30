#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# TokenTree Codex Friend Validation Runner (Linux / macOS)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
CODEX_SESSIONS_DIR="${1:-$HOME/.codex/sessions}"
OUTPUT_DIR="${2:-$SCRIPT_DIR/evidence-codex}"

echo "=== TokenTree OpenAI Codex Validation Runner ==="

TEST_HOME="$OUTPUT_DIR/tokentree-home"
rm -rf "$OUTPUT_DIR"
mkdir -p "$TEST_HOME"

echo "Compiling TokenTree CLI..."
(cd "$REPO_ROOT" && cargo build -p tokentree-cli)
TOKENTREE_BIN="$REPO_ROOT/target/debug/tokentree"

if [ -d "$CODEX_SESSIONS_DIR" ]; then
    echo "Importing Codex sessions from $CODEX_SESSIONS_DIR..."
    "$TOKENTREE_BIN" --home "$TEST_HOME" import codex "$CODEX_SESSIONS_DIR" || true
fi

echo "Running Doctor..."
DOCTOR_OUT=$("$TOKENTREE_BIN" --home "$TEST_HOME" doctor)

echo "Running Reconcile..."
RECONCILE_OUT=$("$TOKENTREE_BIN" --home "$TEST_HOME" reconcile)

echo "Generating Report..."
REPORT_OUT=$("$TOKENTREE_BIN" --home "$TEST_HOME" report --text)

EVIDENCE_FILE="$OUTPUT_DIR/codex-validation-evidence.json"
node -e '
  const fs = require("fs");
  const data = {
    timestamp: new Date().toISOString(),
    host: {
      os: process.platform,
      arch: process.arch,
      codex_path: process.env.CODEX_SESSIONS_DIR
    },
    doctor: process.env.DOCTOR_OUT,
    reconcile: process.env.RECONCILE_OUT,
    report: process.env.REPORT_OUT
  };
  fs.writeFileSync(process.env.EVIDENCE_FILE, JSON.stringify(data, null, 2));
' CODEX_SESSIONS_DIR="$CODEX_SESSIONS_DIR" DOCTOR_OUT="$DOCTOR_OUT" RECONCILE_OUT="$RECONCILE_OUT" REPORT_OUT="$REPORT_OUT" EVIDENCE_FILE="$EVIDENCE_FILE"

echo "Validation evidence saved to: $EVIDENCE_FILE"
echo "=== Validation Completed Successfully ==="
