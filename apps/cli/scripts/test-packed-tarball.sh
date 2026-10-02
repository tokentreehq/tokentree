#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Repeatable packed-tarball distribution test for @tokentreehq/cli.
#
# Replicates the release workflow's npm staging locally
# (.github/workflows/release.yml: "Stage native binaries and checksums" +
# "Stage publishable package" + "Pack and smoke-test exact npm artifact")
# and proves the real published artifact works end to end:
#   1. Stage vendor/<target>/ with the locally built native binary,
#      generate + verify SHA256SUMS.txt (as the workflow does).
#   2. Stage the publishable package exactly like the workflow
#      (dist + data + migrations + vendor + README + LICENSE, scripts and
#      devDependencies stripped, files list set).
#   3. `npm pack` and assert the tarball contains dist/main.js AND the
#      vendored binary.
#   4. `npm install --global` the tarball into an isolated prefix and run the
#      installed bin's ACTUAL dist/main.js: --version and doctor must delegate
#      to the vendored native binary.
#   5. Negative: with vendor/ removed, the same packed dist/main.js must fail
#      fast with the intended clear error (C8).
#
# NOTE: only the host platform's binary can be built locally; the workflow
# builds all five targets in CI. The mechanics verified here (staging layout,
# tarball contents, launcher discovery, delegation) are platform-independent.
#
# Run:  pnpm --filter @tokentreehq/cli test:packed-tarball
set -euo pipefail

CLI_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$(cd "$CLI_ROOT/../.." && pwd)"
WORK="$(mktemp -d)"
export WORK CLI_ROOT
trap 'rm -rf "$WORK"' EXIT

TARGET="x86_64-unknown-linux-gnu"
BIN_NAME="tokentree"

pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1"; exit 1; }

echo "== 1. build CLI package =="
(cd "$REPO_ROOT" && pnpm --filter @tokentreehq/cli build >/dev/null)
test -f "$CLI_ROOT/dist/main.js" || fail "dist/main.js missing after build"
pass "CLI built"

echo "== 2. native binary present =="
NATIVE_BIN="$REPO_ROOT/target/release/$BIN_NAME"
test -x "$NATIVE_BIN" || fail "missing $NATIVE_BIN (run: cargo build --release -p tokentree-cli)"
pass "native binary: $("$NATIVE_BIN" --version)"

echo "== 3. stage vendor/ like the release workflow =="
rm -rf "$WORK/npm-vendor" && mkdir -p "$WORK/npm-vendor/$TARGET"
cp "$NATIVE_BIN" "$WORK/npm-vendor/$TARGET/"
chmod +x "$WORK/npm-vendor/$TARGET/$BIN_NAME"
(cd "$WORK/npm-vendor" && find . -type f ! -name SHA256SUMS.txt -print0 | sort -z | xargs -0 sha256sum > SHA256SUMS.txt)
(cd "$WORK/npm-vendor" && sha256sum -c SHA256SUMS.txt >/dev/null)
pass "vendor/$TARGET/$BIN_NAME staged, SHA256SUMS.txt verified"

echo "== 4. stage publishable package like the release workflow =="
rm -rf "$WORK/npm-stage" && mkdir -p "$WORK/npm-stage"
cp -r "$CLI_ROOT/dist" "$CLI_ROOT/data" "$CLI_ROOT/migrations" "$WORK/npm-vendor" "$WORK/npm-stage/"
mv "$WORK/npm-stage/npm-vendor" "$WORK/npm-stage/vendor"
cp "$CLI_ROOT/README.md" "$CLI_ROOT/LICENSE" "$WORK/npm-stage/"
node - <<'NODE'
const fs = require('node:fs');
const stage = process.env.WORK + '/npm-stage';
const pkg = JSON.parse(fs.readFileSync(process.env.CLI_ROOT + '/package.json', 'utf8'));
delete pkg.devDependencies;
delete pkg.scripts;
pkg.files = ['dist', 'migrations', 'data', 'vendor', 'README.md', 'LICENSE'];
fs.writeFileSync(stage + '/package.json', JSON.stringify(pkg, null, 2) + '\n');
NODE
test -f "$WORK/npm-stage/dist/main.js"
test -f "$WORK/npm-stage/data/prices.json" || fail "data/prices.json missing (C5 regression)"
test -f "$WORK/npm-stage/vendor/$TARGET/$BIN_NAME"
pass "staged package matches workflow layout"

echo "== 5. npm pack =="
(cd "$WORK/npm-stage" && tarball=$(npm pack --silent) && echo "$tarball" > "$WORK/tarball-name")
TARBALL="$WORK/npm-stage/$(cat "$WORK/tarball-name")"
echo "tarball path: [$TARBALL]"
ls -la "$TARBALL" || fail "tarball file missing at $TARBALL"
# NOTE: list to a file first — `tar -tf | grep -q` under `set -o pipefail`
# misfires because grep -q closes the pipe early (tar dies of SIGPIPE).
tar -tf "$TARBALL" > "$WORK/tarlist.txt"
grep -q 'package/dist/main.js' "$WORK/tarlist.txt" || { head -20 "$WORK/tarlist.txt"; fail "tarball missing dist/main.js"; }
grep -q "package/vendor/$TARGET/$BIN_NAME" "$WORK/tarlist.txt" || fail "tarball missing vendored binary"
pass "tarball contains dist/main.js + vendored binary ($(basename "$TARBALL"))"

echo "== 6. global install from tarball + run ACTUAL packed dist/main.js =="
PREFIX="$WORK/prefix"
npm install --global --prefix "$PREFIX" "$TARBALL" >/dev/null 2>&1
test -x "$PREFIX/bin/tokentree" || fail "$PREFIX/bin/tokentree not installed"
export TOKENTREE_HOME="$WORK/home"
"$PREFIX/bin/tokentree" --version || fail "--version failed through packed launcher"
"$PREFIX/bin/tokentree" doctor || fail "doctor failed through packed launcher"
# Prove the packed launcher really used the VENDORED binary (not PATH): the
# binary it delegates to must be the one inside the installed package.
test -x "$PREFIX/lib/node_modules/@tokentreehq/cli/vendor/$TARGET/$BIN_NAME" \
  || fail "vendored binary not found in installed package"
pass "packed dist/main.js delegates to vendored native binary (--version, doctor)"

echo "== 7. negative: packed launcher fails fast with no binary (C8) =="
mv "$PREFIX/lib/node_modules/@tokentreehq/cli/vendor" "$WORK/vendor-hidden"
if "$PREFIX/bin/tokentree" --version >"$WORK/fail.log" 2>&1; then
  mv "$WORK/vendor-hidden" "$PREFIX/lib/node_modules/@tokentreehq/cli/vendor"
  fail "launcher should exit nonzero without a native binary"
fi
mv "$WORK/vendor-hidden" "$PREFIX/lib/node_modules/@tokentreehq/cli/vendor"
grep -q "no native tokentree binary found" "$WORK/fail.log" || { cat "$WORK/fail.log"; fail "missing clear error"; }
pass "fail-fast error verified on packed artifact"

echo ""
echo "ALL PACKED-TARBALL CHECKS PASSED"
