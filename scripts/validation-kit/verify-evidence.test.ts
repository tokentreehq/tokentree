// SPDX-License-Identifier: Apache-2.0
import { writeFileSync, unlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { verifyEvidenceFile } from './verify-evidence.js';

describe('Friend validation kit evidence verifier', () => {
  it('accepts clean evidence without secrets', () => {
    const tempPath = join(tmpdir(), `test-clean-${Date.now()}.json`);
    const cleanData = {
      timestamp: new Date().toISOString(),
      host: { os: 'windows', arch: 'x64' },
      doctor: 'database: ok\nprompt leakage: 0',
      reconcile: 'sessions: 1\nduplicate canonical request IDs: 0',
      report: 'requests: 1 measured 1',
    };
    writeFileSync(tempPath, JSON.stringify(cleanData));

    try {
      const res = verifyEvidenceFile(tempPath);
      expect(res.valid).toBe(true);
      expect(res.errors).toHaveLength(0);
    } finally {
      unlinkSync(tempPath);
    }
  });

  it('rejects evidence with injected API keys', () => {
    const tempPath = join(tmpdir(), `test-dirty-${Date.now()}.json`);
    const dirtyData = {
      timestamp: new Date().toISOString(),
      host: { os: 'windows' },
      doctor: 'sk-ant-api03-01234567890123456789012345',
      reconcile: 'sessions: 1',
      report: 'requests: 1',
    };
    writeFileSync(tempPath, JSON.stringify(dirtyData));

    try {
      const res = verifyEvidenceFile(tempPath);
      expect(res.valid).toBe(false);
      expect(res.errors.length).toBeGreaterThan(0);
    } finally {
      unlinkSync(tempPath);
    }
  });
});
