// SPDX-License-Identifier: Apache-2.0
import { createHash } from 'node:crypto';
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';

export interface PlatformTarget {
  readonly platform: string;
  readonly arch: string;
  readonly binaryName: string;
  readonly rustTarget: string;
}

export const SUPPORTED_TARGETS: Record<string, PlatformTarget> = {
  'darwin-arm64': {
    platform: 'darwin',
    arch: 'arm64',
    binaryName: 'tokentree',
    rustTarget: 'aarch64-apple-darwin',
  },
  'darwin-x64': {
    platform: 'darwin',
    arch: 'x64',
    binaryName: 'tokentree',
    rustTarget: 'x86_64-apple-darwin',
  },
  'linux-x64': {
    platform: 'linux',
    arch: 'x64',
    binaryName: 'tokentree',
    rustTarget: 'x86_64-unknown-linux-gnu',
  },
  'linux-arm64': {
    platform: 'linux',
    arch: 'arm64',
    binaryName: 'tokentree',
    rustTarget: 'aarch64-unknown-linux-gnu',
  },
  'win32-x64': {
    platform: 'win32',
    arch: 'x64',
    binaryName: 'tokentree.exe',
    rustTarget: 'x86_64-pc-windows-msvc',
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
  const baseDir = options.baseDir ?? import.meta.dirname;
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

export function computeSha256(content: Buffer | string): string {
  return createHash('sha256').update(content).digest('hex');
}

export function verifyBinaryChecksum(filePath: string, expectedSha256: string): boolean {
  if (!existsSync(filePath)) return false;
  const content = readFileSync(filePath);
  const actualHash = computeSha256(content);
  return actualHash.toLowerCase() === expectedSha256.toLowerCase();
}
