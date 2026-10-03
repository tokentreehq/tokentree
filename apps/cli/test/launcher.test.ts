// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import {
  computeSha256,
  findNativeBinary,
  findOptionalDependencyBinary,
  formatMissingBinaryMessage,
  resolvePlatformTarget,
  SUPPORTED_TARGETS,
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

  it('locates native binary in archive layout vendor directory with SHA-256 verification', () => {
    const tmp = mkdtempSync(join(tmpdir(), 'tokentree-vendor-test-'));
    try {
      const vendorDir = join(tmp, 'vendor', 'x86_64-pc-windows-msvc');
      mkdirSync(vendorDir, { recursive: true });
      const binPath = join(vendorDir, 'tokentree.exe');
      const binPayload = Buffer.from('mock windows release candidate binary content');
      writeFileSync(binPath, binPayload);

      const found = findNativeBinary({
        baseDir: tmp,
        platform: 'win32',
        arch: 'x64',
      });
      expect(found).toBe(binPath);

      const expectedSha256 = computeSha256(binPayload);
      expect(verifyBinaryChecksum(found!, expectedSha256)).toBe(true);

      const tamperedSha256 = '1111111111111111111111111111111111111111111111111111111111111111';
      expect(verifyBinaryChecksum(found!, tamperedSha256)).toBe(false);
    } finally {
      rmSync(tmp, { recursive: true, force: true });
    }
  });
});

describe('launcher platform sub-packages (audit v3 C1)', () => {
  it('maps every supported target to its optional-dependency npm package', () => {
    const cases: Array<[string, string, string]> = [
      ['darwin', 'arm64', '@tokentreehq/cli-darwin-arm64'],
      ['darwin', 'x64', '@tokentreehq/cli-darwin-x64'],
      ['linux', 'x64', '@tokentreehq/cli-linux-x64'],
      ['linux', 'arm64', '@tokentreehq/cli-linux-arm64'],
      ['win32', 'x64', '@tokentreehq/cli-win32-x64'],
    ];
    for (const [platform, arch, npmPackage] of cases) {
      expect(resolvePlatformTarget(platform, arch)?.npmPackage).toBe(npmPackage);
    }
    // The SUPPORTED_TARGETS keys double as the npm package suffixes.
    for (const [key, target] of Object.entries(SUPPORTED_TARGETS)) {
      expect(target.npmPackage).toBe(`@tokentreehq/cli-${key}`);
    }
  });

  /** Build a fake installed optional-dep package: <tmp>/node_modules/<scope>/<name>/{package.json,bin/<bin>}. */
  function stageOptionalDep(tmp: string, npmPackage: string, binName: string): string {
    const [scope, name] = npmPackage.split('/');
    const pkgDir = join(tmp, 'node_modules', scope, name);
    const binDir = join(pkgDir, 'bin');
    mkdirSync(binDir, { recursive: true });
    writeFileSync(join(pkgDir, 'package.json'), JSON.stringify({ name: npmPackage, version: '0.0.0-test' }));
    const binPath = join(binDir, binName);
    writeFileSync(binPath, 'mock platform binary');
    return binPath;
  }

  it('resolves the installed optional dependency before the vendor fallback', () => {
    const tmp = mkdtempSync(join(tmpdir(), 'tokentree-optdep-test-'));
    try {
      const target = resolvePlatformTarget('win32', 'x64')!;
      const optDepBin = stageOptionalDep(tmp, target.npmPackage, target.binaryName);
      // A vendor/ binary exists too — the optional dep must win.
      const vendorDir = join(tmp, 'vendor', target.rustTarget);
      mkdirSync(vendorDir, { recursive: true });
      writeFileSync(join(vendorDir, target.binaryName), 'mock vendor binary');

      expect(findOptionalDependencyBinary(tmp, target)).toBe(optDepBin);
      expect(findNativeBinary({ baseDir: tmp, platform: 'win32', arch: 'x64' })).toBe(optDepBin);
    } finally {
      rmSync(tmp, { recursive: true, force: true });
    }
  });

  it('falls back to vendor/ when the optional dep has no binary staged', () => {
    const tmp = mkdtempSync(join(tmpdir(), 'tokentree-optdep-empty-test-'));
    try {
      const target = resolvePlatformTarget('linux', 'x64')!;
      // Optional dep installed but bin/ missing (e.g. dev checkout link) —
      // resolution must skip it and use the vendor fallback.
      const [scope, name] = target.npmPackage.split('/');
      const pkgDir = join(tmp, 'node_modules', scope, name);
      mkdirSync(pkgDir, { recursive: true });
      writeFileSync(join(pkgDir, 'package.json'), JSON.stringify({ name: target.npmPackage }));
      expect(findOptionalDependencyBinary(tmp, target)).toBeUndefined();

      const vendorDir = join(tmp, 'vendor', target.rustTarget);
      mkdirSync(vendorDir, { recursive: true });
      const vendorBin = join(vendorDir, target.binaryName);
      writeFileSync(vendorBin, 'mock vendor binary');
      expect(findNativeBinary({ baseDir: tmp, platform: 'linux', arch: 'x64' })).toBe(vendorBin);
    } finally {
      rmSync(tmp, { recursive: true, force: true });
    }
  });

  it('returns undefined when neither optional dep nor fallback layouts exist', () => {
    const tmp = mkdtempSync(join(tmpdir(), 'tokentree-optdep-missing-test-'));
    try {
      const target = resolvePlatformTarget('darwin', 'arm64')!;
      expect(findOptionalDependencyBinary(tmp, target)).toBeUndefined();
      expect(findNativeBinary({ baseDir: tmp, platform: 'darwin', arch: 'arm64' })).toBeUndefined();
    } finally {
      rmSync(tmp, { recursive: true, force: true });
    }
  });
});

describe('launcher missing-binary message (audit v3 C3)', () => {
  it('keeps the fail-fast lead line on every platform', () => {
    for (const [platform, arch] of [['linux', 'x64'], ['win32', 'x64'], ['darwin', 'arm64']] as const) {
      expect(formatMissingBinaryMessage({ platform, arch, baseDir: '/tmp/x' }))
        .toContain('tokentree: no native tokentree binary found');
    }
  });

  it('shows Windows path style on win32', () => {
    const msg = formatMissingBinaryMessage({
      platform: 'win32',
      arch: 'x64',
      appDataDir: 'C:\\Users\\tester\\AppData\\Roaming',
    });
    expect(msg).toContain('C:\\Users\\tester\\AppData\\Roaming\\npm\\node_modules\\@tokentreehq\\cli\\vendor\\x86_64-pc-windows-msvc');
    expect(msg).toContain('tokentree.exe');
    expect(msg).toContain('TOKENTREE_BIN=C:\\path\\to\\tokentree.exe');
    expect(msg).not.toContain('vendor/');
  });

  it('falls back to the %APPDATA% placeholder when APPDATA is unset', () => {
    const msg = formatMissingBinaryMessage({ platform: 'win32', arch: 'x64' });
    // appDataDir omitted -> process.env.APPDATA or the literal placeholder.
    expect(msg).toMatch(/%APPDATA%|AppData/);
    expect(msg).toContain('npm\\node_modules\\@tokentreehq\\cli\\vendor\\');
  });

  it('keeps Unix-style vendor paths off Windows', () => {
    const msg = formatMissingBinaryMessage({ platform: 'linux', arch: 'x64', baseDir: '/tmp/x' });
    expect(msg).toContain(join('/tmp/x', '..', 'vendor', 'x86_64-unknown-linux-gnu'));
    expect(msg).toContain('TOKENTREE_BIN=/path/to/tokentree');
  });
});
