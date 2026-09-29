// SPDX-License-Identifier: Apache-2.0
import { readFileSync } from 'node:fs';

export interface HookEnvelope {
  readonly version: 1;
  readonly kind: string;
  readonly capturedAt: string;
  readonly payload: Readonly<Record<string, unknown>>;
}

export interface HookRecordBatch {
  readonly records: readonly HookEnvelope[];
  readonly consumedBytes: number;
  readonly skipped: number;
}

export function optionalText(value: unknown): string | undefined {
  return typeof value === 'string' && value.length > 0 ? value : undefined;
}

export function readCompleteHookRecords(path: string, startOffset: number): HookRecordBatch {
  const buffer = readFileSync(path);
  const safeOffset = Math.min(startOffset, buffer.length);
  const remaining = buffer.subarray(safeOffset).toString('utf8');
  const lines = remaining.split('\n');

  // A hook may be interrupted while appending. Leave the incomplete tail for replay.
  if (!remaining.endsWith('\n')) lines.pop();

  const records: HookEnvelope[] = [];
  let consumedBytes = 0;
  let skipped = 0;

  for (const line of lines) {
    consumedBytes += Buffer.byteLength(line, 'utf8') + 1;
    if (line.trim() === '') continue;

    try {
      const candidate = JSON.parse(line) as Partial<HookEnvelope>;
      if (candidate.version !== 1 || typeof candidate.kind !== 'string' || !candidate.payload) {
        skipped++;
        continue;
      }
      records.push(candidate as HookEnvelope);
    } catch {
      skipped++;
    }
  }

  return { records, consumedBytes, skipped };
}
