// SPDX-License-Identifier: Apache-2.0
import { createHash } from 'node:crypto';
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';

export interface PlatformTarget {
  readonly platform: string;
  readonly arch: string;
  readonly binaryName: string;
  readonly rustTarget: string;
  readonly npmPackage: string;
}

export const SUPPORTED_TARGETS: Record<string, PlatformTarget> = {
  'darwin-arm64': {
    platform: 'darwin',
    arch: 'arm64',
    binaryName: 'tokentree',
    rustTarget: 'aarch64-apple-darwin',
    npmPackage: '@tokentreehq/cli-darwin-arm64',
  },
  'darwin-x64': {
    platform: 'darwin',
    arch: 'x64',
    binaryName: 'tokentree',
    rustTarget: 'x86_64-apple-darwin',
    npmPackage: '@tokentreehq/cli-darwin-x64',
  },
  'linux-x64': {
    platform: 'linux',
    arch: 'x64',
    binaryName: 'tokentree',
    rustTarget: 'x86_64-unknown-linux-gnu',
    npmPackage: '@tokentreehq/cli-linux-x64',
  },
  'linux-arm64': {
    platform: 'linux',
    arch: 'arm64',
    binaryName: 'tokentree',
    rustTarget: 'aarch64-unknown-linux-gnu',
    npmPackage: '@tokentreehq/cli-linux-arm64',
  },
  'win32-x64': {
    platform: 'win32',
    arch: 'x64',
    binaryName: 'tokentree.exe',
    rustTarget: 'x86_64-pc-windows-msvc',
    npmPackage: '@tokentreehq/cli-win32-x64',
  },
};

export function resolvePlatformTarget(platform: string, arch: string): PlatformTarget | null {
  const key = `${platform}-${arch}`;
  return SUPPORTED_TARGETS[key] ?? null;
}

export interface FindBinaryOptions {
  readonly envBin?: string;
  readonly baseDir?: string;
  readonly platform?: string;
  readonly arch?: string;
}

export function findOptionalDependencyBinary(baseDir: string, target: PlatformTarget): string | undefined {
  const slashIdx = target.npmPackage.indexOf('/');
  const scope = slashIdx >= 0 ? target.npmPackage.slice(0, slashIdx) : '';
  const name = slashIdx >= 0 ? target.npmPackage.slice(slashIdx + 1) : target.npmPackage;

  const candidates = [
    join(baseDir, 'node_modules', scope, name, 'bin', target.binaryName),
    join(baseDir, '..', 'node_modules', scope, name, 'bin', target.binaryName),
    join(baseDir, '..', '..', 'node_modules', scope, name, 'bin', target.binaryName),
  ];
  for (const c of candidates) {
    if (existsSync(c)) return c;
  }
  return undefined;
}

export function findNativeBinary(options: FindBinaryOptions = {}): string | undefined {
  const envBin = options.envBin ?? process.env.TOKENTREE_BIN;
  if (envBin && existsSync(envBin)) {
    return envBin;
  }
  const platform = options.platform ?? process.platform;
  const arch = options.arch ?? process.arch;
  const target = resolvePlatformTarget(platform, arch);
  const ext = platform === 'win32' ? '.exe' : '';
  const binName = target?.binaryName ?? `tokentree${ext}`;
  const baseDir = options.baseDir ?? import.meta.dirname ?? process.cwd();

  if (target) {
    const optDepBin = findOptionalDependencyBinary(baseDir, target);
    if (optDepBin) return optDepBin;
  }

  const candidates = [
    join(baseDir, '..', 'bin', binName),
    ...(target ? [
      join(baseDir, '..', 'vendor', target.rustTarget, binName),
      join(baseDir, 'vendor', target.rustTarget, binName),
    ] : []),
    join(baseDir, '..', '..', '..', 'target', 'release', binName),
    join(baseDir, '..', '..', '..', 'target', 'debug', binName),
  ];
  for (const c of candidates) {
    if (existsSync(c)) return c;
  }
  return undefined;
}

export interface FormatMissingBinaryOptions {
  readonly platform?: string;
  readonly arch?: string;
  readonly baseDir?: string;
  readonly appDataDir?: string;
}

export function formatMissingBinaryMessage(options: FormatMissingBinaryOptions = {}): string {
  const platform = options.platform ?? process.platform;
  const arch = options.arch ?? process.arch;
  const target = resolvePlatformTarget(platform, arch);

  if (platform === 'win32') {
    const appData = options.appDataDir ?? process.env.APPDATA ?? '%APPDATA%';
    const vendorDir = `${appData}\\npm\\node_modules\\@tokentreehq\\cli\\vendor\\${target?.rustTarget ?? 'x86_64-pc-windows-msvc'}`;
    const binName = target?.binaryName ?? 'tokentree.exe';
    return (
      'tokentree: no native tokentree binary found for this platform.\n' +
      'The @tokentreehq/cli package is a launcher for the Rust measurement engine and cannot run without it.\n' +
      'Install the native binary:\n' +
      '  1. Download the archive for your platform from https://github.com/tokentreehq/tokentree/releases\n' +
      `  2. Extract the \`${binName}\` binary into ${vendorDir}\n` +
      '     (or set TOKENTREE_BIN=C:\\path\\to\\tokentree.exe to point at an existing binary).'
    );
  }

  const baseDir = options.baseDir ?? import.meta.dirname ?? process.cwd();
  const vendorDir = target
    ? join(baseDir, '..', 'vendor', target.rustTarget)
    : join(baseDir, '..', 'vendor');
  const binName = target?.binaryName ?? 'tokentree';
  return (
    'tokentree: no native tokentree binary found for this platform.\n' +
    'The @tokentreehq/cli package is a launcher for the Rust measurement engine and cannot run without it.\n' +
    'Install the native binary:\n' +
    '  1. Download the archive for your platform from https://github.com/tokentreehq/tokentree/releases\n' +
    `  2. Extract the \`${binName}\` binary into ${vendorDir}\n` +
    '     (or set TOKENTREE_BIN=/path/to/tokentree to point at an existing binary).'
  );
}

export function computeSha256(content: Buffer | string): string {
  return createHash('sha256').update(content).digest('hex');
}

export function verifyBinaryChecksum(filePath: string, expectedSha256: string): boolean {
  if (!existsSync(filePath)) return false;
  const content = readFileSync(filePath);
  const actualHash = computeSha256(content);
  return actualHash.toLowerCase() === expectedSha256.toLowerCase();
}
