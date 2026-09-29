// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import {
  computeSha256,
  findNativeBinary,
  resolvePlatformTarget,
  verifyBinaryChecksum,
} from '../src/launcher.js';

describe('launcher platform target resolution', () => {
  it('resolves all supported OS and architecture targets', () => {
    const targets = [
      { platform: 'darwin', arch: 'arm64', expectedBin: 'tokentree', expectedRust: 'aarch64-apple-darwin' },
      { platform: 'darwin', arch: 'x64', expectedBin: 'tokentree', expectedRust: 'x86_64-apple-darwin' },
      { platform: 'linux', arch: 'x64', expectedBin: 'tokentree', expectedRust: 'x86_64-unknown-linux-gnu' },
      { platform: 'linux', arch: 'arm64', expectedBin: 'tokentree', expectedRust: 'aarch64-unknown-linux-gnu' },
      { platform: 'win32', arch: 'x64', expectedBin: 'tokentree.exe', expectedRust: 'x86_64-pc-windows-msvc' },
    ];

    for (const t of targets) {
      const resolved = resolvePlatformTarget(t.platform, t.arch);
      expect(resolved).not.toBeNull();
      expect(resolved?.binaryName).toBe(t.expectedBin);
      expect(resolved?.rustTarget).toBe(t.expectedRust);
    }
  });

  it('rejects unsupported platforms and architectures', () => {
    expect(resolvePlatformTarget('freebsd', 'x64')).toBeNull();
    expect(resolvePlatformTarget('win32', 'ia32')).toBeNull();
    expect(resolvePlatformTarget('sunos', 'x64')).toBeNull();
    expect(resolvePlatformTarget('linux', 'mips')).toBeNull();
  });
});

describe('launcher checksum verification', () => {
  it('computes sha256 and verifies binary integrity', () => {
    const tmp = mkdtempSync(join(tmpdir(), 'tokentree-launcher-test-'));
    try {
      const mockBin = join(tmp, 'tokentree-mock');
      const payload = Buffer.from('mock binary machine code payload 12345');
      writeFileSync(mockBin, payload);

      const expectedHash = computeSha256(payload);
      expect(expectedHash).toHaveLength(64);

      // Verify matching checksum
      expect(verifyBinaryChecksum(mockBin, expectedHash)).toBe(true);
      expect(verifyBinaryChecksum(mockBin, expectedHash.toUpperCase())).toBe(true);

      // Reject tampered hash
      const tamperedHash = '0000000000000000000000000000000000000000000000000000000000000000';
      expect(verifyBinaryChecksum(mockBin, tamperedHash)).toBe(false);

      // Reject nonexistent file
      expect(verifyBinaryChecksum(join(tmp, 'nonexistent'), expectedHash)).toBe(false);
    } finally {
      rmSync(tmp, { recursive: true, force: true });
    }
  });
});

describe('launcher findNativeBinary', () => {
  it('honors TOKENTREE_BIN environment override if file exists', () => {
    const tmp = mkdtempSync(join(tmpdir(), 'tokentree-env-test-'));
    try {
      const customBin = join(tmp, 'custom-tokentree');
      writeFileSync(customBin, 'mock binary');

      const found = findNativeBinary({ envBin: customBin });
      expect(found).toBe(customBin);
    } finally {
      rmSync(tmp, { recursive: true, force: true });
    }
  });

  it('falls back to undefined if candidate files do not exist', () => {
    const tmp = mkdtempSync(join(tmpdir(), 'tokentree-empty-test-'));
    try {
      const found = findNativeBinary({ envBin: join(tmp, 'nonexistent'), baseDir: tmp });
      expect(found).toBeUndefined();
    } finally {
      rmSync(tmp, { recursive: true, force: true });
    }
  });
});
