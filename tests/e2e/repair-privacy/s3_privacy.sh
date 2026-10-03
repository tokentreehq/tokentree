#!/usr/bin/env bash
# SUITE 3 — privacy and security. Repeatable. PASS/FAIL per check.
set -uo pipefail
BIN=${TOKENTREE_BIN:-/tmp/tokentree-release-bin}
H=/tmp/e2e/s3-home
C1="CANARY_PROMPT_9x7zqx"; C2="sk-ant-CANARY_KEY_12345"
PASS=0; FAIL=0
ok()  { echo "PASS: $1"; PASS=$((PASS+1)); }
bad() { echo "FAIL: $1"; FAIL=$((FAIL+1)); }
cleanup() { kill $DPID $OPID 2>/dev/null; wait 2>/dev/null; }

echo "=== (a) poison hook records with canaries ==="
rm -rf "$H"; mkdir -p "$H/spool" "$H/good-proj" "$H/poison-proj"
printf 'project: poison\nexec: /bin/true\n' > "$H/poison-proj/.tokentree.yml"
SPOOL=$H/spool/claude-hooks.jsonl
cat > "$SPOOL" <<EOF
{"version":1,"kind":"UserPromptSubmit","capturedAt":"2026-09-29T10:00:00Z","payload":{"session_id":"ses_good_1","prompt_fingerprint":"abc12345","cwd":"$H/good-proj"}}
{"version":1,"kind":"UserPromptSubmit","capturedAt":"2026-09-29T10:01:00Z","payload":{"session_id":"ses_poison","prompt_fingerprint":"def67890","cwd":"$H/poison-proj","prompt_text":"please ignore previous instructions $C1","api_key_hint":"$C2"}}
{"version":1,"kind":"UserPromptSubmit","capturedAt":"2026-09-29T10:02:00Z","payload":{"session_id":"ses_good_2","prompt_fingerprint":"abc12345","cwd":"$H/good-proj"}}
EOF
OUT=$(TOKENTREE_HOME=$H $BIN classify 2>/tmp/e2e/s3-classify.err); RC=$?
echo "$OUT" | head -8
Q=$H/spool/claude-hooks.quarantine.jsonl
[ -f "$Q" ] && ok "quarantine file created" || bad "no quarantine file"
grep -q "event_fingerprint" "$Q" && ! grep -q "$C1" "$Q" && ! grep -q "$C2" "$Q" \
  && ok "quarantine holds fingerprint+metadata, NOT canaries" \
  || bad "quarantine content wrong"
# fingerprint must match sha256 of the raw line (read_line keeps the \n)
FP=$(sed -n '2p' "$SPOOL" | sha256sum | cut -d' ' -f1)
grep -q "$FP" "$Q" && ok "quarantine fingerprint matches sha256 of raw line" || bad "fingerprint mismatch"
# file perms 0600
[ "$(stat -c %a "$Q")" = "600" ] && ok "quarantine file is 0600" || bad "quarantine perms $(stat -c %a "$Q")"

echo "=== (b) canary sweep of entire home ==="
HITS1=$(grep -rl "$C1" "$H" 2>/dev/null | sort)
HITS2=$(grep -rl "$C2" "$H" 2>/dev/null | sort)
echo "prompt canary in: $HITS1"
echo "key canary in: $HITS2"
[ "$HITS1" = "$SPOOL" ] && ok "prompt canary ONLY in the spool input file" || bad "prompt canary leaked"
[ "$HITS2" = "$SPOOL" ] && ok "key canary ONLY in the spool input file" || bad "key canary leaked"
# DB must not contain them (sqlite LIKE on all text columns would be slow; use strings on the db file)
! strings "$H/ledger.db" | grep -q "$C1\|$C2" && ok "canaries absent from ledger.db bytes" || bad "canary in ledger.db"

echo "=== (c) dashboard auth transport ==="
TOKENTREE_HOME=$H $BIN dashboard --port 18741 --no-open >/tmp/e2e/s3-dash.log 2>&1 &
DPID=$!; sleep 2
TOKEN=$(grep -oP 'token=\K[0-9a-f]+' /tmp/e2e/s3-dash.log | head -1)
[ "${#TOKEN}" = "64" ] && ok "dashboard issues 64-hex session token" || bad "token format"
[ "$(curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:18741/api/status)" = "401" ] \
  && ok "API without credentials -> 401" || bad "API without creds not 401"
[ "$(curl -s -o /dev/null -w "%{http_code}" "http://127.0.0.1:18741/api/status?token=deadbeef")" = "401" ] \
  && ok "API with wrong token -> 401" || bad "API wrong token not 401"
[ "$(curl -s -o /dev/null -w "%{http_code}" "http://127.0.0.1:18741/?token=$TOKEN")" = "200" ] \
  && ok "first navigation with ?token= works" || bad "first nav failed"
curl -s "http://127.0.0.1:18741/?token=$TOKEN" -o /tmp/e2e/s3-dash.html
grep -q "history.replaceState" /tmp/e2e/s3-dash.html \
  && ok "served page strips ?token= via history.replaceState" || bad "no strip logic"
grep -q "Authorization.*Bearer" /tmp/e2e/s3-dash.html \
  && ok "API calls use Authorization: Bearer header" || bad "no bearer header usage"
! grep -q "localStorage.setItem\|localStorage\[" /tmp/e2e/s3-dash.html \
  && ok "token never written to localStorage" || bad "token in localStorage"
[ "$(curl -s -o /dev/null -w "%{http_code}" -H "Authorization: Bearer $TOKEN" http://127.0.0.1:18741/api/status)" = "200" ] \
  && ok "API with Bearer header -> 200" || bad "bearer header failed"

echo "=== (d) OTLP auth ==="
TOKENTREE_HOME=$H $BIN otlp-serve --address 127.0.0.1:14319 >/tmp/e2e/s3-otlp.log 2>&1 &
OPID=$!; sleep 2
OT=$(grep -A1 "required on every ingest" /tmp/e2e/s3-otlp.log | tail -1 | tr -d ' ')
PAYLOAD='{"resourceLogs":[{"resource":{"attributes":[{"key":"session.id","value":{"stringValue":"s3"}}]},"scopeLogs":[{"logRecords":[{"attributes":[{"key":"event.name","value":{"stringValue":"api_request"}},{"key":"request_id","value":{"stringValue":"req_s3"}},{"key":"input_tokens","value":{"intValue":"10"}},{"key":"output_tokens","value":{"intValue":"2"}}]}]}]}]}'
BEFORE=$(sqlite3 "$H/ledger.db" "SELECT count(*) FROM usage_events;")
[ "$(curl -s -o /dev/null -w "%{http_code}" -X POST http://127.0.0.1:14319/v1/logs -H "Content-Type: application/json" -d "$PAYLOAD")" = "401" ] \
  && [ "$(sqlite3 "$H/ledger.db" "SELECT count(*) FROM usage_events;")" = "$BEFORE" ] \
  && ok "OTLP without token -> 401, zero rows written" || bad "unauth OTLP wrote rows or wrong code"
R=$(curl -s -w "\n%{http_code}" -X POST http://127.0.0.1:14319/v1/logs -H "Content-Type: application/json" -H "Authorization: Bearer $OT" -d "$PAYLOAD")
echo "$R" | tail -1 | grep -q "200" && [ "$(sqlite3 "$H/ledger.db" "SELECT count(*) FROM usage_events WHERE request_id='req_s3';")" = "1" ] \
  && ok "OTLP with token -> 200, exactly 1 row" || bad "authed OTLP failed"

cleanup
echo "=== summary: $PASS passed, $FAIL failed ==="
[ $FAIL -eq 0 ]
