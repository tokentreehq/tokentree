// SPDX-License-Identifier: Apache-2.0
import { readFileSync, existsSync } from 'node:fs';

export const EVIDENCE_SCHEMA_VERSION = '1.0.0';
export const ALLOWED_ADAPTERS = ['claude', 'codex', 'grok', 'hermes'] as const;

export interface ValidationEvidenceEnvironment {
  readonly isolated_workspace: boolean;
  readonly isolated_home: boolean;
  readonly clean_baseline: boolean;
}

export interface ValidationEvidenceChecks {
  readonly live_capture_verified: boolean;
  readonly ledger_created: boolean;
  readonly doctor_clean: boolean;
  readonly no_leaks_detected: boolean;
  readonly reconcile_clean: boolean;
}

export interface ValidationEvidenceCounters {
  readonly total_sessions: number;
  readonly total_turns: number;
  readonly total_requests: number;
  readonly measured_requests: number;
  readonly unavailable_requests: number;
  readonly anomalous_requests: number;
  readonly duplicate_requests: number;
  readonly unresolved_anomalies: number;
  readonly total_tokens: number;
  readonly input_tokens: number;
  readonly output_tokens: number;
  readonly cache_read_tokens: number;
  readonly cache_write_tokens: number;
  readonly reasoning_tokens: number;
  readonly cost_micros: number;
}

export interface ValidationEvidence {
  readonly schema_version: typeof EVIDENCE_SCHEMA_VERSION;
  readonly adapter: (typeof ALLOWED_ADAPTERS)[number];
  readonly environment: ValidationEvidenceEnvironment;
  readonly checks: ValidationEvidenceChecks;
  readonly counters: ValidationEvidenceCounters;
}

const FORBIDDEN_PATTERNS = [
  /sk-ant-[a-zA-Z0-9_-]{20,}/,
  /sk-[a-zA-Z0-9_-]{20,}/,
  /Bearer\s+[a-zA-Z0-9_.-]{20,}/i,
  /x-api-key/i,
  /authorization:\s*bearer/i,
  /[a-zA-Z]:\\[Uu]sers\\/,
  /\/home\/[a-zA-Z0-9_.-]+/,
  /\/Users\/[a-zA-Z0-9_.-]+/,
];

const ROOT_ALLOWED_KEYS = new Set(['schema_version', 'adapter', 'environment', 'checks', 'counters']);
const ENV_ALLOWED_KEYS = new Set(['isolated_workspace', 'isolated_home', 'clean_baseline']);
const CHECKS_ALLOWED_KEYS = new Set([
  'live_capture_verified',
  'ledger_created',
  'doctor_clean',
  'no_leaks_detected',
  'reconcile_clean',
]);
const COUNTERS_ALLOWED_KEYS = new Set([
  'total_sessions',
  'total_turns',
  'total_requests',
  'measured_requests',
  'unavailable_requests',
  'anomalous_requests',
  'duplicate_requests',
  'unresolved_anomalies',
  'total_tokens',
  'input_tokens',
  'output_tokens',
  'cache_read_tokens',
  'cache_write_tokens',
  'reasoning_tokens',
  'cost_micros',
]);

const FORBIDDEN_KEY_SUBSTRINGS = [
  'prompt',
  'completion',
  'stdout',
  'stderr',
  'output',
  'command',
  'username',
  'user',
  'path',
  'home',
  'cwd',
  'dir',
  'file',
  'source',
  'raw',
  'secret',
  'key',
  'token',
];

function assertNoUnknownKeys(obj: Record<string, unknown>, allowed: Set<string>, prefix: string, errors: string[]) {
  for (const key of Object.keys(obj)) {
    if (!allowed.has(key)) {
      errors.push(`Forbidden unknown field '${prefix}${key}' is not in allowlisted schema`);
    }
  }
}

function checkSensitiveKeys(obj: unknown, prefix: string, errors: string[]) {
  if (typeof obj !== 'object' || obj === null) return;
  if (Array.isArray(obj)) {
    errors.push(`Arrays are not permitted in evidence schema at '${prefix}'`);
    return;
  }
  for (const [key, value] of Object.entries(obj)) {
    const fullPath = prefix ? `${prefix}.${key}` : key;
    const lowerKey = key.toLowerCase();
    for (const forbidden of FORBIDDEN_KEY_SUBSTRINGS) {
      if (lowerKey.includes(forbidden)) {
        const isPermittedKey =
          ROOT_ALLOWED_KEYS.has(key) ||
          ENV_ALLOWED_KEYS.has(key) ||
          CHECKS_ALLOWED_KEYS.has(key) ||
          COUNTERS_ALLOWED_KEYS.has(key);
        if (!isPermittedKey) {
          errors.push(`Forbidden field '${fullPath}' contains sensitive keyword '${forbidden}'`);
        }
      }
    }
    if (typeof value === 'object' && value !== null) {
      checkSensitiveKeys(value, fullPath, errors);
    }
  }
}

export function verifyEvidenceData(data: unknown): { valid: boolean; errors: string[] } {
  const errors: string[] = [];

  if (typeof data !== 'object' || data === null || Array.isArray(data)) {
    return { valid: false, errors: ['Evidence root must be an object'] };
  }

  const root = data as Record<string, unknown>;

  // 1. Recursive unknown field rejection
  assertNoUnknownKeys(root, ROOT_ALLOWED_KEYS, '', errors);
  checkSensitiveKeys(root, '', errors);

  // 2. Schema version verification
  if (root.schema_version !== EVIDENCE_SCHEMA_VERSION) {
    errors.push(
      `Invalid schema_version '${String(root.schema_version)}'. Expected '${EVIDENCE_SCHEMA_VERSION}'`
    );
  }

  // 3. Adapter verification
  if (
    typeof root.adapter !== 'string' ||
    !ALLOWED_ADAPTERS.includes(root.adapter as (typeof ALLOWED_ADAPTERS)[number])
  ) {
    errors.push(`Invalid adapter '${String(root.adapter)}'. Expected one of: ${ALLOWED_ADAPTERS.join(', ')}`);
  }

  // 4. Environment verification
  if (typeof root.environment !== 'object' || root.environment === null || Array.isArray(root.environment)) {
    errors.push("Missing or invalid 'environment' object");
  } else {
    const env = root.environment as Record<string, unknown>;
    assertNoUnknownKeys(env, ENV_ALLOWED_KEYS, 'environment.', errors);
    for (const key of ENV_ALLOWED_KEYS) {
      if (typeof env[key] !== 'boolean') {
        errors.push(`Field 'environment.${key}' must be a boolean`);
      }
    }
  }

  // 5. Checks verification
  if (typeof root.checks !== 'object' || root.checks === null || Array.isArray(root.checks)) {
    errors.push("Missing or invalid 'checks' object");
  } else {
    const chk = root.checks as Record<string, unknown>;
    assertNoUnknownKeys(chk, CHECKS_ALLOWED_KEYS, 'checks.', errors);
    for (const key of CHECKS_ALLOWED_KEYS) {
      if (typeof chk[key] !== 'boolean') {
        errors.push(`Field 'checks.${key}' must be a boolean`);
      }
    }
  }

  // 6. Counters verification
  if (typeof root.counters !== 'object' || root.counters === null || Array.isArray(root.counters)) {
    errors.push("Missing or invalid 'counters' object");
  } else {
    const cnt = root.counters as Record<string, unknown>;
    assertNoUnknownKeys(cnt, COUNTERS_ALLOWED_KEYS, 'counters.', errors);
    for (const key of COUNTERS_ALLOWED_KEYS) {
      const val = cnt[key];
      if (typeof val !== 'number' || !Number.isSafeInteger(val) || val < 0) {
        errors.push(`Field 'counters.${key}' must be a non-negative integer`);
      }
    }
  }

  return {
    valid: errors.length === 0,
    errors,
  };
}

export function verifyEvidenceFile(filePath: string): { valid: boolean; errors: string[] } {
  const errors: string[] = [];
  if (!existsSync(filePath)) {
    return { valid: false, errors: [`File not found: ${filePath}`] };
  }

  const content = readFileSync(filePath, 'utf8');

  // Check raw content for forbidden sensitive patterns
  for (const pattern of FORBIDDEN_PATTERNS) {
    if (pattern.test(content)) {
      errors.push(`Found forbidden sensitive pattern matching ${pattern}`);
    }
  }

  try {
    const data = JSON.parse(content);
    const dataResult = verifyEvidenceData(data);
    errors.push(...dataResult.errors);
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
    console.log(`[PASS] Evidence file ${target} is clean, allowlisted, and contains zero leaks/secrets.`);
  } else {
    console.error(`[FAIL] Verification errors in ${target}:`);
    for (const err of result.errors) {
      console.error(`  - ${err}`);
    }
    process.exit(1);
  }
}
