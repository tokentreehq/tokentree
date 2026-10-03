#!/usr/bin/env bash
# SUITE 2 — every provider produces trees. Repeatable. PASS/FAIL per provider.
set -uo pipefail
BIN=${TOKENTREE_BIN:-/tmp/tokentree-release-bin}
# Fixture dir, repo-relative (was a machine-specific absolute path).
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
F=${E2E_FIXTURES:-$SCRIPT_DIR/../../../fixtures/parsers}
PASS=0; FAIL=0
ok()  { echo "PASS: $1"; PASS=$((PASS+1)); }
bad() { echo "FAIL: $1"; FAIL=$((FAIL+1)); }

test_provider() { # name, import-subcommand, fixture-path
  local name=$1 cmd=$2 fix=$3
  local H=/tmp/e2e/s2-$name DB
  DB=$H/ledger.db
  echo "=== provider: $name ==="
  rm -rf "$H"; mkdir -p "$H"
  if ! TOKENTREE_HOME=$H $BIN import "$cmd" "$fix" >/tmp/e2e/s2-$name-import.json 2>&1; then
    bad "$name: import failed"; cat /tmp/e2e/s2-$name-import.json; return
  fi
  local ns ne nsp nag
  ns=$(sqlite3 "$DB" "SELECT count(*) FROM sessions;")
  ne=$(sqlite3 "$DB" "SELECT count(*) FROM usage_events WHERE superseded_by IS NULL;")
  nsp=$(sqlite3 "$DB" "SELECT count(*) FROM usage_spans;")
  nag=$(sqlite3 "$DB" "SELECT count(*) FROM attribution_groups WHERE active=1;")
  [ "$ns" -gt 0 ] && [ "$ne" -gt 0 ] && [ "$nsp" -gt 0 ] && [ "$nag" -gt 0 ] \
    && ok "$name: sessions=$ns events=$ne spans=$nsp active_attr_groups=$nag" \
    || { bad "$name: missing rows (s=$ns e=$ne sp=$nsp ag=$nag)"; return; }
  local badbp
  badbp=$(sqlite3 "$DB" "SELECT count(*) FROM (SELECT g.id FROM attribution_groups g JOIN attributions a ON a.group_id=g.id WHERE g.active=1 GROUP BY g.id HAVING sum(a.weight_basis_points) != 10000);")
  [ "$badbp" = "0" ] && ok "$name: all active groups sum to 10000bp" || bad "$name: $badbp groups violate 10000bp"
  local rep1
  rep1=$(TOKENTREE_HOME=$H $BIN report --text 2>&1)
  echo "$rep1" | grep -q "tok" && [ "$(sqlite3 "$DB" "SELECT coalesce(sum(input_tokens),0)+coalesce(sum(output_tokens),0)+coalesce(sum(cached_input_tokens),0)+coalesce(sum(cache_write_tokens),0) FROM usage_events WHERE superseded_by IS NULL;")" -gt 0 ] \
    && ok "$name: report --text renders tree with non-zero tokens" \
    || { bad "$name: empty tree"; echo "$rep1" | head -5; return; }
  local before after
  before=$(sqlite3 "$DB" "SELECT coalesce(sum(input_tokens),0),coalesce(sum(output_tokens),0),count(*) FROM usage_events WHERE superseded_by IS NULL;")
  TOKENTREE_HOME=$H $BIN import "$cmd" "$fix" >/dev/null 2>&1
  after=$(sqlite3 "$DB" "SELECT coalesce(sum(input_tokens),0),coalesce(sum(output_tokens),0),count(*) FROM usage_events WHERE superseded_by IS NULL;")
  [ "$before" = "$after" ] && ok "$name: second import creates no duplicate totals" || bad "$name: duplicate totals ($before -> $after)"
}

test_provider claude claude "$F/claude/public-small-v2.1.80.jsonl"
test_provider codex  codex  "$F/codex/public-small-codex-clean.jsonl"
# grok discovers files named usage.json (real CLI layout)
rm -rf /tmp/e2e/grokdata; mkdir -p /tmp/e2e/grokdata/sess1
cp "$F/grok/multi-turn.json" /tmp/e2e/grokdata/sess1/usage.json
test_provider grok   grok   /tmp/e2e/grokdata
test_provider hermes hermes "$F/hermes/oneshot-usage.json"

echo "=== summary: $PASS passed, $FAIL failed ==="
[ $FAIL -eq 0 ]
