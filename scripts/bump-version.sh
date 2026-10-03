#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Bump the TokenTree version across every versioned manifest in lockstep
# (audit v3 C21). The release workflow enforces tag <-> package version match;
# this script keeps the four manifests in sync with each other so the plugin
# versions (C15) cannot drift from the CLI/workspace version again.
#
# Usage: scripts/bump-version.sh <x.y.z>
set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "usage: $0 <x.y.z>" >&2
  exit 1
fi

new_version="$1"
if [[ ! "$new_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "error: '$new_version' is not a valid semver x.y.z version" >&2
  exit 1
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# Bump the first "version" value in a JSON file, preserving formatting.
bump_json() {
  local file="$1" before after
  before="$(node -p "JSON.parse(require('fs').readFileSync('$file', 'utf8')).version")"
  node -e "
const fs = require('fs');
const path = '$file';
let text = fs.readFileSync(path, 'utf8');
const re = /(\"version\"\s*:\s*\")[^\"]*(\")/;
if (!re.test(text)) { console.error('no version field found in ' + path); process.exit(1); }
fs.writeFileSync(path, text.replace(re, '\$1$new_version\$2'));
"
  after="$(node -p "JSON.parse(require('fs').readFileSync('$file', 'utf8')).version")"
  echo "$file: $before -> $after"
}

# Bump version inside the [workspace.package] section of Cargo.toml only
# (dependency versions elsewhere in the file must not be touched).
bump_cargo() {
  local file="Cargo.toml" before after
  before="$(sed -n '/^\[workspace\.package\]/,/^\[/ s/^version = "\(.*\)"/\1/p' "$file" | head -1)"
  perl -i -pe 'if (/^\[workspace\.package\]/ ... /^\[/) { s/^(version\s*=\s*")[^"]*(")$/${1}'"$new_version"'$2/ }' "$file"
  after="$(sed -n '/^\[workspace\.package\]/,/^\[/ s/^version = "\(.*\)"/\1/p' "$file" | head -1)"
  if [ "$before" = "$after" ]; then
    echo "error: failed to bump version in $file" >&2
    exit 1
  fi
  echo "$file ([workspace.package]): $before -> $after"
}

bump_json "apps/cli/package.json"
bump_cargo
bump_json "plugins/claude-code/package.json"
bump_json "plugins/claude-code/.claude-plugin/plugin.json"
# Platform sub-packages (audit v3 C1): keep in lockstep to avoid cosmetic
# drift; release.yml also stamps them from apps/cli/package.json at publish.
for target in linux-x64 linux-arm64 darwin-x64 darwin-arm64 win32-x64; do
  bump_json "packages/cli-$target/package.json"
done

echo "bumped all manifests to $new_version"
