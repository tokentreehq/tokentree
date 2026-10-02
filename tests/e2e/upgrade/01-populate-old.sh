#!/usr/bin/env bash
# Populate a ledger with the OLD (pre-PR#6) binary: snapshots, multi-adapter
# sessions, truth-ladder pairs, manual corrections, custom source_kind.
set -euo pipefail
OLD_BIN=/tmp/tokentree-old-bin
export TOKENTREE_HOME=/tmp/e2e/home-old
mkdir -p "$TOKENTREE_HOME"

$OLD_BIN import claude /tmp/e2e/inputs/claude/batch1
$OLD_BIN import codex  /tmp/e2e/inputs/codex
$OLD_BIN import grok    /tmp/e2e/inputs/grok
$OLD_BIN import hermes  /tmp/e2e/inputs/hermes
# second batch: provider_fields row for the same request as a transcript row
$OLD_BIN import claude /tmp/e2e/inputs/claude/batch2

# manual session + corrections
OUT=$($OLD_BIN start --project "e2e-proj" --task "manual work item")
WI=$(echo "$OUT" | python3 -c "import json,sys; print(json.load(sys.stdin)['work_item_id'])")
SES=$(echo "$OUT" | python3 -c "import json,sys; print(json.load(sys.stdin)['session_id'])")
$OLD_BIN stop --input 100 --output 50
$OLD_BIN rename --task "$WI" --title "renamed work item"
$OLD_BIN note --task "$WI" --text "e2e baseline note"
$OLD_BIN attach --session "$SES" --task "$WI"
SPAN=$(sqlite3 "$TOKENTREE_HOME/ledger.db" "SELECT id FROM usage_spans LIMIT 1;")
$OLD_BIN split --source "$WI" --title "split child" --spans "$SPAN"

# custom source_kind row (bypasses parser)
sqlite3 "$TOKENTREE_HOME/ledger.db" "INSERT INTO usage_events (id, adapter, source_kind, source_event_id, session_id, observed_at, ingested_at, input_tokens, output_tokens, event_hash, adapter_version, parser_version) VALUES ('evt_custom_001','claude','my_custom_kind','custom-evt-1',(SELECT id FROM sessions LIMIT 1),'2026-09-03T10:00:00Z','2026-09-03T10:00:01Z',10,5,'customhash001','0.2.0','0.2.0-rust');"
echo "populate done"
