// SPDX-License-Identifier: Apache-2.0
import { appendFileSync, chmodSync, mkdirSync, openSync, closeSync } from 'node:fs';
import { dirname } from 'node:path';

export interface SpoolEnvelope { readonly version: 1; readonly kind: string; readonly capturedAt: string; readonly payload: Readonly<Record<string, unknown>>; }

export function enqueueSpool(path: string, envelope: SpoolEnvelope): void {
  mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
  const fd = openSync(path, 'a', 0o600);
  try { appendFileSync(fd, `${JSON.stringify(envelope)}\n`, 'utf8'); }
  finally { closeSync(fd); }
  try { chmodSync(path, 0o600); } catch { /* Filesystem may not expose POSIX modes. */ }
}
