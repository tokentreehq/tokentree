#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# TokenTree Claude Code Friend Validation Runner (Linux / macOS)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OUTPUT_DIR="${1:-$SCRIPT_DIR/evidence-claude}"

echo "=== TokenTree Claude Code Validation Runner ==="

if ! command -v claude &> /dev/null; then
    echo "Error: Claude CLI ('claude') not found in PATH." >&2
    exit 1
fi

CLAUDE_VERSION=$(claude --version)
echo "Detected Claude CLI: $CLAUDE_VERSION"

TEST_HOME="$OUTPUT_DIR/tokentree-home"
TEST_WORKSPACE="$OUTPUT_DIR/test-workspace"
rm -rf "$OUTPUT_DIR"
mkdir -p "$TEST_HOME" "$TEST_WORKSPACE"

cat << 'EOF' > "$TEST_WORKSPACE/package.json"
{
  "name": "tokentree-live-validation",
  "version": "1.0.0",
  "private": true
}
EOF

echo "Compiling TokenTree CLI..."
(cd "$REPO_ROOT" && cargo build -p tokentree-cli)
TOKENTREE_BIN="$REPO_ROOT/target/debug/tokentree"

echo "Executing live Claude test prompt..."
(cd "$TEST_WORKSPACE" && claude -p "Reply with exactly: TokenTree live capture verified.")

echo "Processing capture..."
"$TOKENTREE_BIN" --home "$TEST_HOME" classify || true

echo "Running Doctor..."
DOCTOR_OUT=$("$TOKENTREE_BIN" --home "$TEST_HOME" doctor)

echo "Running Reconcile..."
RECONCILE_OUT=$("$TOKENTREE_BIN" --home "$TEST_HOME" reconcile)

echo "Generating Report..."
REPORT_OUT=$("$TOKENTREE_BIN" --home "$TEST_HOME" report --text)

EVIDENCE_FILE="$OUTPUT_DIR/claude-validation-evidence.json"
node -e '
  const fs = require("fs");
  const data = {
    timestamp: new Date().toISOString(),
    host: {
      os: process.platform,
      arch: process.arch,
      claude_version: process.env.CLAUDE_VERSION
    },
    doctor: process.env.DOCTOR_OUT,
    reconcile: process.env.RECONCILE_OUT,
    report: process.env.REPORT_OUT
  };
  fs.writeFileSync(process.env.EVIDENCE_FILE, JSON.stringify(data, null, 2));
' CLAUDE_VERSION="$CLAUDE_VERSION" DOCTOR_OUT="$DOCTOR_OUT" RECONCILE_OUT="$RECONCILE_OUT" REPORT_OUT="$REPORT_OUT" EVIDENCE_FILE="$EVIDENCE_FILE"

echo "Validation evidence saved to: $EVIDENCE_FILE"
echo "=== Validation Completed Successfully ==="
