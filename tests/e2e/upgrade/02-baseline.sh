#!/usr/bin/env bash
# Capture baseline state. Args: <home> <outdir>
set -euo pipefail
HOME_DIR=$1; OUT=$2; BIN=${3:-/tmp/tokentree-old-bin}
export TOKENTREE_HOME="$HOME_DIR"; DB="$HOME_DIR/ledger.db"
mkdir -p "$OUT"
$BIN report --text > "$OUT/report.txt"
for t in usage_events sessions turns usage_spans work_items projects attributions attribution_groups notes measurement_anomalies; do
  printf "%s: %s\n" "$t" "$(sqlite3 "$DB" "SELECT count(*) FROM $t;")"
done > "$OUT/counts.txt"
sqlite3 "$DB" "SELECT DISTINCT source_kind FROM usage_events ORDER BY 1;" > "$OUT/kinds.txt"
cp "$DB" "$OUT/ledger.db"
echo "baseline captured in $OUT"
