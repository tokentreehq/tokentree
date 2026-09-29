#!/bin/sh
# SPDX-License-Identifier: Apache-2.0
set -eu
BINARY="${CLAUDE_PLUGIN_ROOT}/bin/tokentree"
if [ -x "$BINARY" ]; then
  exec "$BINARY" hook-enqueue
fi
# Development checkout fallback only. Release packaging must include the Rust binary.
exec node "${CLAUDE_PLUGIN_ROOT}/scripts/enqueue.mjs"
