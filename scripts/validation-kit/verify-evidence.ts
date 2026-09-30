// SPDX-License-Identifier: Apache-2.0
import { readFileSync, existsSync } from 'node:fs';

const forbiddenPatterns = [
  /sk-ant-[a-zA-Z0-9_\-]{20,}/,
  /sk-[a-zA-Z0-9_\-]{20,}/,
  /Bearer\s+[a-zA-Z0-9_.\-]{20,}/i,
  /x-api-key/i,
  /authorization:\s*bearer/i,
];

export function verifyEvidenceFile(filePath: string): { valid: boolean; errors: string[] } {
  const errors: string[] = [];
  if (!existsSync(filePath)) {
    return { valid: false, errors: [`File not found: ${filePath}`] };
  }

  const content = readFileSync(filePath, 'utf8');

  for (const pattern of forbiddenPatterns) {
    if (pattern.test(content)) {
      errors.push(`Found forbidden sensitive pattern matching ${pattern}`);
    }
  }

  try {
    const data = JSON.parse(content);
    if (!data.timestamp) errors.push('Missing timestamp');
    if (!data.host) errors.push('Missing host info');
    if (!data.doctor) errors.push('Missing doctor output');
    if (!data.reconcile) errors.push('Missing reconcile output');
    if (!data.report) errors.push('Missing report output');
  } catch (e) {
    errors.push(`Invalid JSON format: ${String(e)}`);
  }

  return {
    valid: errors.length === 0,
    errors,
  };
}

if (process.argv[1] && process.argv[1].endsWith('verify-evidence.ts')) {
  const target = process.argv[2];
  if (!target) {
    console.error('Usage: tsx verify-evidence.ts <path-to-evidence.json>');
    process.exit(1);
  }
  const result = verifyEvidenceFile(target);
  if (result.valid) {
    console.log(`[PASS] Evidence file ${target} is clean and contains zero leaks/secrets.`);
  } else {
    console.error(`[FAIL] Verification errors in ${target}:`);
    for (const err of result.errors) {
      console.error(`  - ${err}`);
    }
    process.exit(1);
  }
}
