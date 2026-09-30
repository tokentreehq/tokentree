// SPDX-License-Identifier: Apache-2.0
import { writeFileSync, unlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { verifyEvidenceFile, EVIDENCE_SCHEMA_VERSION } from './verify-evidence.js';

function validEvidence() {
  return {
    schema_version: EVIDENCE_SCHEMA_VERSION,
    adapter: 'claude',
    environment: {
      isolated_workspace: true,
      isolated_home: true,
      clean_baseline: true,
    },
    checks: {
      live_capture_verified: true,
      ledger_created: true,
      doctor_clean: true,
      no_leaks_detected: true,
      reconcile_clean: true,
    },
    counters: {
      total_sessions: 1,
      total_turns: 1,
      total_requests: 1,
      measured_requests: 1,
      unavailable_requests: 0,
      anomalous_requests: 0,
      duplicate_requests: 0,
      unresolved_anomalies: 0,
      total_tokens: 1500,
      input_tokens: 1200,
      output_tokens: 300,
      cache_read_tokens: 0,
      cache_write_tokens: 0,
      reasoning_tokens: 0,
      cost_micros: 4500,
    },
  };
}

describe('Friend validation kit evidence verifier', () => {
  it('accepts clean versioned allowlisted evidence', () => {
    const tempPath = join(tmpdir(), `test-clean-${Date.now()}.json`);
    writeFileSync(tempPath, JSON.stringify(validEvidence(), null, 2));

    try {
      const res = verifyEvidenceFile(tempPath);
      expect(res.valid).toBe(true);
      expect(res.errors).toHaveLength(0);
    } finally {
      unlinkSync(tempPath);
    }
  });

  it('rejects evidence with injected API keys', () => {
    const tempPath = join(tmpdir(), `test-dirty-key-${Date.now()}.json`);
    const dirty = validEvidence() as Record<string, unknown>;
    dirty.notes = 'sk-ant-api03-01234567890123456789012345';
    writeFileSync(tempPath, JSON.stringify(dirty));

    try {
      const res = verifyEvidenceFile(tempPath);
      expect(res.valid).toBe(false);
      expect(res.errors.some(e => e.includes('forbidden sensitive pattern') || e.includes('unknown field'))).toBe(true);
    } finally {
      unlinkSync(tempPath);
    }
  });

  it('rejects raw prompt injection', () => {
    const tempPath = join(tmpdir(), `test-dirty-prompt-${Date.now()}.json`);
    const dirty = validEvidence() as Record<string, unknown>;
    dirty.prompt = 'What is the secret formula?';
    writeFileSync(tempPath, JSON.stringify(dirty));

    try {
      const res = verifyEvidenceFile(tempPath);
      expect(res.valid).toBe(false);
      expect(res.errors.some(e => e.includes('Forbidden unknown field') || e.includes('sensitive keyword'))).toBe(true);
    } finally {
      unlinkSync(tempPath);
    }
  });

  it('rejects raw completion and stdout injection', () => {
    const tempPath = join(tmpdir(), `test-dirty-completion-${Date.now()}.json`);
    const dirty = validEvidence() as Record<string, unknown>;
    dirty.completion = 'Here is the secret completion.';
    dirty.stdout = 'Command executed successfully';
    writeFileSync(tempPath, JSON.stringify(dirty));

    try {
      const res = verifyEvidenceFile(tempPath);
      expect(res.valid).toBe(false);
      expect(res.errors.some(e => e.includes('Forbidden unknown field'))).toBe(true);
    } finally {
      unlinkSync(tempPath);
    }
  });

  it('rejects usernames, home directories, and filesystem paths', () => {
    const tempPath = join(tmpdir(), `test-dirty-paths-${Date.now()}.json`);
    const dirty = validEvidence() as Record<string, unknown>;
    dirty.user_info = { username: 'alice', home_path: 'C:\\Users\\alice\\tokentree' };
    writeFileSync(tempPath, JSON.stringify(dirty));

    try {
      const res = verifyEvidenceFile(tempPath);
      expect(res.valid).toBe(false);
      expect(res.errors.some(e => e.includes('forbidden sensitive pattern') || e.includes('Forbidden unknown field'))).toBe(true);
    } finally {
      unlinkSync(tempPath);
    }
  });

  it('rejects unknown fields recursively in nested objects', () => {
    const tempPath = join(tmpdir(), `test-dirty-nested-${Date.now()}.json`);
    const dirty = validEvidence() as Record<string, unknown>;
    const checks = dirty.checks as Record<string, unknown>;
    const counters = dirty.counters as Record<string, unknown>;
    checks.extra_check_flag = true;
    counters.raw_token_dump = 42;
    writeFileSync(tempPath, JSON.stringify(dirty));

    try {
      const res = verifyEvidenceFile(tempPath);
      expect(res.valid).toBe(false);
      expect(res.errors.some(e => e.includes('checks.extra_check_flag'))).toBe(true);
      expect(res.errors.some(e => e.includes('counters.raw_token_dump'))).toBe(true);
    } finally {
      unlinkSync(tempPath);
    }
  });

  it('rejects string values in aggregate counters', () => {
    const tempPath = join(tmpdir(), `test-dirty-type-${Date.now()}.json`);
    const dirty = validEvidence() as Record<string, unknown>;
    const counters = dirty.counters as Record<string, unknown>;
    counters.total_tokens = '1500'; // string instead of number
    writeFileSync(tempPath, JSON.stringify(dirty));

    try {
      const res = verifyEvidenceFile(tempPath);
      expect(res.valid).toBe(false);
      expect(res.errors.some(e => e.includes('counters.total_tokens'))).toBe(true);
    } finally {
      unlinkSync(tempPath);
    }
  });

  it('rejects invalid or unsupported schema versions', () => {
    const tempPath = join(tmpdir(), `test-dirty-version-${Date.now()}.json`);
    const dirty = validEvidence() as Record<string, unknown>;
    dirty.schema_version = '9.9.9';
    writeFileSync(tempPath, JSON.stringify(dirty));

    try {
      const res = verifyEvidenceFile(tempPath);
      expect(res.valid).toBe(false);
      expect(res.errors.some(e => e.includes('Invalid schema_version'))).toBe(true);
    } finally {
      unlinkSync(tempPath);
    }
  });
});
