#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
import { spawnSync } from 'node:child_process';
import { findNativeBinary, formatMissingBinaryMessage } from './launcher.js';

// The TypeScript CLI is a launcher for the Rust measurement engine — it is
// not a second implementation. Per the locked Rust-first architecture decision
// (docs/decisions.md), it must fail clearly when no supported native binary
// is present and must never silently fall back to a JS engine.
const nativeBin = findNativeBinary();
if (!nativeBin) {
  console.error(formatMissingBinaryMessage());
  process.exit(1);
}

const isCmd = process.platform === 'win32' && /\.(cmd|bat)$/i.test(nativeBin);
const result = spawnSync(nativeBin, process.argv.slice(2), { stdio: 'inherit', shell: isCmd });
process.exit(result.status ?? (result.signal ? 1 : 0));
